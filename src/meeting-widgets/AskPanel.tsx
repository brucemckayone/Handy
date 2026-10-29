import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowUp,
  Check,
  ChevronDown,
  Copy,
  FileText,
  Folder,
  GitMerge,
  Image,
  Mic,
  MessageCircleQuestion,
  MessageSquareShare,
  Plus,
  RotateCcw,
  Square,
  SquareTerminal,
  TextSelect,
  Trash2,
  TriangleAlert,
  AppWindow,
  Volume2,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  commands,
  events,
  type Answer,
  type Exchange,
  type Thread,
  type ThreadListItem,
} from "@/bindings";
import { MarkdownContent } from "@/components/whats-new/MarkdownContent";
import { useOsType } from "@/hooks/useOsType";
import { keycapLabel } from "@/lib/utils/keyboard";

/** An answer's spoken part, and its detail once the `---` line arrived. */
const splitAnswer = (text: string): [string, string | null] => {
  let start = 0;
  for (const line of text.split(/(?<=\n)/)) {
    const end = start + line.length;
    if (line.endsWith("\n") && line.trim() === "---") {
      return [text.slice(0, start).trimEnd(), text.slice(end).trimStart()];
    }
    start = end;
  }
  return [text.trimEnd(), null];
};

const folderName = (path: string) =>
  path
    .replace(/[\\/]+$/, "")
    .split(/[\\/]/)
    .pop() ?? path;

const iconButton =
  "w-9 h-9 shrink-0 flex items-center justify-center rounded-[10px] text-ink-2 hover:bg-wash transition-colors cursor-pointer";
const smallIconButton =
  "w-8 h-8 shrink-0 flex items-center justify-center rounded-lg text-ink-2 hover:bg-wash transition-colors cursor-pointer";
const chip =
  "h-6 inline-flex items-center gap-1.5 px-2 rounded-md bg-wash text-xs text-ink-2 max-w-full";
const secondaryButton =
  "h-[30px] px-3 rounded-lg border border-line-strong/40 bg-background text-[13px] hover:bg-wash transition-colors cursor-pointer";

const useAgo = () => {
  const { t, i18n } = useTranslation();
  return (at: number) => {
    const minutes = Math.round((Date.now() - at) / 60000);
    if (minutes < 1) return t("ask.panel.now");
    const format = new Intl.RelativeTimeFormat(i18n.language, {
      numeric: "auto",
    });
    if (minutes < 60) return format.format(-minutes, "minute");
    const hours = Math.round(minutes / 60);
    if (hours < 24) return format.format(-hours, "hour");
    return format.format(-Math.round(hours / 24), "day");
  };
};

/** The Ask panel: one thread's questions and answers, and a box to type a
 * follow-up. Opened from the speaking pill or the tray; Esc closes it. */
export const AskPanel: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [threads, setThreads] = useState<ThreadListItem[]>([]);
  const [thread, setThread] = useState<Thread | null>(null);
  const [streams, setStreams] = useState<Record<string, string>>({});
  const [menuOpen, setMenuOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [speaking, setSpeaking] = useState(false);
  const [shortcut, setShortcut] = useState("");
  const [keepDays, setKeepDays] = useState(7);
  const threadId = useRef<string | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const bottom = useRef<HTMLDivElement>(null);

  const showThread = useCallback(async (id: string | null) => {
    const list = await commands.listAskThreads();
    const all = list.status === "ok" ? list.data : [];
    setThreads(all);
    const target = id ?? all[0]?.id ?? null;
    threadId.current = target;
    commands.setAskPanelThread(target);
    if (!target) {
      setThread(null);
      return;
    }
    const found = await commands.getAskThread(target);
    setThread(found.status === "ok" ? found.data : null);
  }, []);

  const startNewThread = () => {
    threadId.current = null;
    commands.setAskPanelThread(null);
    setThread(null);
    setMenuOpen(false);
    input.current?.focus();
  };

  useEffect(() => {
    const refreshThreads = () =>
      commands.listAskThreads().then((list) => {
        if (list.status === "ok") setThreads(list.data);
      });
    commands.getAskPanelThread().then(showThread);
    commands.isSpeaking().then(setSpeaking);
    commands.getAppSettings().then((result) => {
      if (result.status !== "ok") return;
      setShortcut(result.data.bindings?.ask?.current_binding ?? "");
      setKeepDays(result.data.ask?.keep_days ?? 7);
    });
    const unlisteners = [
      events.askOpenEvent.listen(({ payload }) => {
        setMenuOpen(false);
        showThread(payload.thread_id);
        setTimeout(() => input.current?.focus(), 50);
      }),
      events.askThreadEvent.listen(({ payload }) => {
        const changed = payload.thread;
        refreshThreads();
        if (changed.id === threadId.current) setThread(changed);
      }),
      events.askDeltaEvent.listen(({ payload }) => {
        setStreams((s) => ({
          ...s,
          [payload.answer_id]: payload.reset
            ? payload.text
            : (s[payload.answer_id] ?? "") + payload.text,
        }));
      }),
      events.askDeletedEvent.listen(({ payload }) => {
        if (payload.thread_id === threadId.current) showThread(null);
        else refreshThreads();
      }),
      events.speechStateEvent.listen(({ payload }) =>
        setSpeaking(payload.speaking),
      ),
    ];
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") commands.closeAskPanel();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      unlisteners.forEach((u) => u.then((fn) => fn()));
    };
  }, [showThread]);

  const exchanges = thread?.exchanges ?? [];
  const lastAnswer = exchanges[exchanges.length - 1]?.answer;
  const streamedLength = lastAnswer ? (streams[lastAnswer.id] ?? "").length : 0;
  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [thread?.id, exchanges.length, streamedLength]);

  const send = async () => {
    const text = draft.trim();
    if (!text) return;
    setDraft("");
    const result = await commands.askTyped(threadId.current, text);
    if (result.status === "ok") showThread(result.data);
  };

  const shortcutLabel = shortcut ? keycapLabel(shortcut, osType) : "";
  const agent = thread?.agent;

  return (
    <div className="w-full h-full flex flex-col relative rounded-2xl border border-line bg-background text-text font-ui text-sm overflow-hidden">
      <div className="shrink-0 px-2 pt-2.5 pb-2 flex flex-col gap-2">
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setMenuOpen((open) => !open)}
            aria-label={t("ask.panel.switchThread")}
            aria-expanded={menuOpen}
            className="flex-1 min-w-0 h-9 flex items-center gap-2 px-2 rounded-[10px] hover:bg-wash transition-colors cursor-pointer text-start"
          >
            <span className="w-6 h-6 shrink-0 rounded-full bg-logo-primary flex items-center justify-center">
              <MessageCircleQuestion
                className="w-3.5 h-3.5 text-plum"
                aria-hidden="true"
              />
            </span>
            <span className="min-w-0 truncate font-semibold">
              {thread?.title ?? t("ask.panel.newThread")}
            </span>
            <ChevronDown
              className="w-4 h-4 shrink-0 text-ink-2"
              aria-hidden="true"
            />
          </button>
          <button
            type="button"
            onClick={startNewThread}
            aria-label={t("ask.panel.newThread")}
            title={t("ask.panel.newThread")}
            className={iconButton}
          >
            <Plus className="w-[18px] h-[18px]" aria-hidden="true" />
          </button>
          <button
            type="button"
            onClick={() => commands.closeAskPanel()}
            aria-label={t("ask.panel.close")}
            title={t("ask.panel.close")}
            className={iconButton}
          >
            <X className="w-[18px] h-[18px]" aria-hidden="true" />
          </button>
        </div>
        {thread && (
          <div className="flex flex-wrap gap-1.5 px-1">
            {thread.app_label && (
              <span className={chip} title={thread.app_label}>
                <AppWindow className="w-3 h-3 shrink-0" aria-hidden="true" />
                <span className="truncate max-w-[220px]">
                  {thread.app_label}
                </span>
              </span>
            )}
            {thread.project_dir && (
              <span className={chip} title={thread.project_dir}>
                <Folder className="w-3 h-3 shrink-0" aria-hidden="true" />
                {folderName(thread.project_dir)}
              </span>
            )}
            {agent?.linked && (
              <span className="h-6 inline-flex items-center gap-1.5 ps-2 pe-0.5 rounded-md bg-moss-wash text-xs text-moss">
                <span className="w-[7px] h-[7px] rounded-full bg-moss" />
                {t("ask.panel.agent", { name: agent.name })}
                <button
                  type="button"
                  onClick={() => commands.unlinkAskAgent(thread.id)}
                  aria-label={t("ask.panel.unlink")}
                  title={t("ask.panel.unlink")}
                  className="w-[22px] h-[22px] flex items-center justify-center rounded hover:bg-moss/15 cursor-pointer"
                >
                  <X className="w-3 h-3" aria-hidden="true" />
                </button>
              </span>
            )}
            {agent && !agent.linked && (
              <span className="h-6 inline-flex items-center px-2 rounded-md border border-dashed border-line-strong/60 text-xs text-ink-2">
                {t("ask.panel.unlinked")}
              </span>
            )}
          </div>
        )}
      </div>

      {menuOpen && (
        <ThreadMenu
          threads={threads}
          current={thread?.id ?? null}
          keepDays={keepDays}
          onPick={(id) => {
            setMenuOpen(false);
            showThread(id);
          }}
          onDelete={(id) => commands.deleteAskThread(id)}
        />
      )}

      <div className="flex-1 min-h-0 overflow-y-auto border-t border-line p-3.5 flex flex-col gap-3.5">
        {thread?.merge && (
          <div className="rounded-xl border border-line bg-background p-3 flex flex-col gap-2">
            <div className="flex gap-2 items-start text-[13px] leading-[18px]">
              <GitMerge
                className="w-4 h-4 shrink-0 mt-px text-background-ui"
                aria-hidden="true"
              />
              <span>
                {t("ask.panel.mergeSuggestion", { title: thread.merge.title })}
              </span>
            </div>
            <div className="flex gap-1.5 justify-end">
              <button
                type="button"
                onClick={() => commands.keepAskThreadsApart(thread.id)}
                className={secondaryButton}
              >
                {t("ask.panel.keepApart")}
              </button>
              <button
                type="button"
                onClick={() => commands.mergeAskThreads(thread.id)}
                className="h-[30px] px-3 rounded-lg bg-background-ui text-white text-[13px] font-semibold hover:opacity-90 cursor-pointer"
              >
                {t("ask.panel.merge")}
              </button>
            </div>
          </div>
        )}

        {agent?.linked && (
          <div className="rounded-[10px] bg-wash px-2.5 py-2 text-[13px] leading-[18px]">
            {t("ask.panel.agentNotice", { name: agent.name })}
          </div>
        )}

        {!thread && (
          <div className="flex-1 flex flex-col items-center justify-center gap-2.5 text-center px-6">
            <span className="w-12 h-12 rounded-full bg-logo-primary/30 flex items-center justify-center">
              <Mic className="w-6 h-6 text-background-ui" aria-hidden="true" />
            </span>
            <span className="text-[17px] font-semibold">
              {t("ask.panel.emptyTitle")}
            </span>
            <span className="text-[13px] leading-[19px] text-ink-2">
              {t("ask.panel.emptyBody", { shortcut: shortcutLabel })}
            </span>
          </div>
        )}

        {thread &&
          exchanges.map((exchange) => (
            <ExchangeView
              key={exchange.question.id}
              thread={thread}
              threads={threads}
              exchange={exchange}
              streamed={streams[exchange.answer.id]}
              speaking={speaking && exchange.answer.id === lastAnswer?.id}
            />
          ))}
        <div ref={bottom} />
      </div>

      {speaking && (
        <div className="shrink-0 mx-3 mb-2 h-10 flex items-center gap-2.5 ps-3 pe-1 rounded-full bg-logo-primary/15 text-[13px]">
          <Volume2 className="w-4 h-4 text-background-ui" aria-hidden="true" />
          <span className="flex-1">{t("ask.panel.speakingBar")}</span>
          <button
            type="button"
            onClick={() => commands.stopSpeaking()}
            className="h-8 px-3 flex items-center gap-1.5 rounded-full bg-background hover:bg-wash cursor-pointer"
          >
            <Square className="w-3 h-3 fill-current" aria-hidden="true" />
            {t("ask.panel.stop")}
          </button>
        </div>
      )}

      <div className="shrink-0 px-3 pb-3 flex flex-col gap-1.5">
        <div className="flex items-end gap-2 p-2 ps-3 rounded-xl border border-line-strong/50 bg-background focus-within:border-background-ui/60">
          <label htmlFor="ask-input" className="sr-only">
            {t("ask.panel.followUpLabel")}
          </label>
          <textarea
            id="ask-input"
            ref={input}
            rows={2}
            value={draft}
            placeholder={
              thread ? t("ask.panel.followUp") : t("ask.panel.firstQuestion")
            }
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            className="flex-1 min-w-0 resize-none bg-transparent outline-none text-sm leading-5 placeholder:text-ink-3"
          />
          <button
            type="button"
            onClick={send}
            disabled={!draft.trim()}
            aria-label={t("ask.panel.send")}
            title={t("ask.panel.send")}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center bg-background-ui text-white disabled:bg-wash disabled:text-ink-3 cursor-pointer disabled:cursor-default"
          >
            <ArrowUp className="w-4 h-4" strokeWidth={2.5} aria-hidden="true" />
          </button>
        </div>
        {shortcutLabel && (
          <span className="flex items-center gap-1.5 px-1 text-xs text-ink-2">
            <Mic className="w-3 h-3" aria-hidden="true" />
            {t("ask.panel.voiceHint", { shortcut: shortcutLabel })}
          </span>
        )}
      </div>
    </div>
  );
};

const ThreadMenu: React.FC<{
  threads: ThreadListItem[];
  current: string | null;
  keepDays: number;
  onPick: (id: string) => void;
  onDelete: (id: string) => void;
}> = ({ threads, current, keepDays, onPick, onDelete }) => {
  const { t } = useTranslation();
  const ago = useAgo();
  return (
    <div className="absolute left-3 right-3 top-[50px] z-10 max-h-[60%] overflow-y-auto rounded-xl border border-line bg-background shadow-lg p-1.5 flex flex-col gap-0.5 animate-drop">
      <span className="px-2 pt-1.5 pb-1 text-xs text-ink-2">
        {t("ask.panel.recent")}
      </span>
      {threads.map((item) => (
        <div
          key={item.id}
          className={`group flex items-center rounded-lg ${item.id === current ? "bg-logo-primary/20" : "hover:bg-wash"}`}
        >
          <button
            type="button"
            onClick={() => onPick(item.id)}
            className="flex-1 min-w-0 flex flex-col p-2 text-start cursor-pointer"
          >
            <span className="font-semibold truncate">{item.title}</span>
            <span className="text-xs text-ink-2 truncate">
              {[item.app_label, ago(item.last_used)]
                .filter(Boolean)
                .join(" · ")}
            </span>
          </button>
          {item.id === current && (
            <Check
              className="w-4 h-4 shrink-0 text-background-ui"
              aria-hidden="true"
            />
          )}
          <button
            type="button"
            onClick={() => onDelete(item.id)}
            aria-label={t("ask.panel.delete")}
            title={t("ask.panel.delete")}
            className={`${smallIconButton} opacity-0 group-hover:opacity-100 focus:opacity-100`}
          >
            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
      ))}
      <span className="px-2 py-1.5 border-t border-line text-xs text-ink-2">
        {keepDays > 0
          ? t("ask.panel.expiry", { count: keepDays })
          : t("ask.panel.expiryNever")}
      </span>
    </div>
  );
};

const ExchangeView: React.FC<{
  thread: Thread;
  threads: ThreadListItem[];
  exchange: Exchange;
  streamed: string | undefined;
  speaking: boolean;
}> = ({ thread, threads, exchange, streamed, speaking }) => {
  const { t } = useTranslation();
  const { question, answer } = exchange;
  const context = question.context;
  return (
    <>
      <div className="flex flex-col items-end gap-1">
        <span className="flex items-center gap-1 text-xs text-ink-2">
          {question.via === "voice" && (
            <Mic className="w-3 h-3" aria-hidden="true" />
          )}
          {t(`ask.panel.via.${question.via}`)}
        </span>
        <div className="max-w-[290px] px-3 py-2 rounded-xl rounded-ee-sm bg-logo-primary/20 whitespace-pre-wrap break-words">
          {question.text}
        </div>
        {context && (
          <div className="flex flex-wrap justify-end gap-1">
            {context.selection_lines != null && (
              <ContextChip icon={TextSelect}>
                {t("ask.panel.chips.selection", {
                  count: context.selection_lines,
                })}
              </ContextChip>
            )}
            {context.input_lines != null && (
              <ContextChip icon={FileText}>
                {t("ask.panel.chips.input", {
                  count: context.input_lines,
                })}
              </ContextChip>
            )}
            {context.screenshot && (
              <ContextChip icon={Image}>
                {t("ask.panel.chips.screenshot")}
              </ContextChip>
            )}
            {context.read_aloud && (
              <ContextChip icon={Volume2}>
                {t("ask.panel.chips.readAloud")}
              </ContextChip>
            )}
            {context.agent && (
              <ContextChip icon={SquareTerminal}>
                {t("ask.panel.agent", { name: context.agent })}
              </ContextChip>
            )}
          </div>
        )}
      </div>
      <AnswerView
        thread={thread}
        threads={threads}
        questionId={question.id}
        answer={answer}
        streamed={streamed}
        speaking={speaking}
      />
    </>
  );
};

const ContextChip: React.FC<{
  icon: LucideIcon;
  children: React.ReactNode;
}> = ({ icon: Icon, children }) => (
  <span className="h-[22px] inline-flex items-center gap-1 px-[7px] rounded-md border border-line bg-background text-xs text-ink-2">
    <Icon className="w-3 h-3" aria-hidden={true} />
    {children}
  </span>
);

const AnswerView: React.FC<{
  thread: Thread;
  threads: ThreadListItem[];
  questionId: string;
  answer: Answer;
  streamed: string | undefined;
  speaking: boolean;
}> = ({ thread, threads, questionId, answer, streamed, speaking }) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [moving, setMoving] = useState(false);
  const live = answer.status === "thinking" || answer.status === "streaming";
  const text = live ? (streamed ?? answer.text) : answer.text;

  if (answer.status === "thinking" && !text) {
    return (
      <div className="flex items-center gap-2 text-[13px] text-ink-2">
        <span aria-hidden="true" className="flex gap-1">
          {[0, 1, 2].map((i) => (
            <span
              key={i}
              className="w-1.5 h-1.5 rounded-full bg-background-ui animate-pulse"
              style={{ animationDelay: `${i * 160}ms` }}
            />
          ))}
        </span>
        {t("ask.panel.thinking")}
      </div>
    );
  }

  if (answer.status === "failed" && answer.error) {
    const code = answer.error.code;
    return (
      <div className="rounded-xl border border-error/30 bg-error-wash p-3 flex flex-col gap-1.5">
        <span className="flex items-center gap-1.5 font-semibold text-error">
          <TriangleAlert className="w-4 h-4" aria-hidden="true" />
          {t(`ask.panel.errors.${code}.title`)}
        </span>
        <span className="text-[13px] leading-[19px]">
          {t(`ask.panel.errors.${code}.body`, {
            detail: answer.error.detail ?? "",
          })}
        </span>
        {code !== "claude_not_found" && (
          <button
            type="button"
            onClick={() =>
              commands.moveAskQuestion(thread.id, questionId, thread.id)
            }
            className={`${secondaryButton} self-start`}
          >
            {t("ask.panel.askAgain")}
          </button>
        )}
      </div>
    );
  }

  const [spoken, detail] = splitAnswer(text);
  // Nothing to say when it was never meant to be read (read aloud off).
  const label = answer.not_spoken
    ? t(`ask.panel.notSpoken.${answer.not_spoken}`)
    : speaking
      ? t("ask.panel.speaking")
      : answer.spoken
        ? t("ask.panel.spoken")
        : null;
  const seconds =
    answer.first_words_ms != null
      ? (answer.first_words_ms / 1000).toFixed(1)
      : null;
  const meta =
    answer.status === "cancelled"
      ? t("ask.panel.stopped")
      : seconds &&
        (answer.model
          ? t("ask.panel.firstWords", {
              model: answer.model[0].toUpperCase() + answer.model.slice(1),
              seconds,
            })
          : t("ask.panel.firstWordsNoModel", { seconds }));

  const copy = async () => {
    const result = await commands.copyMeetingText(text);
    if (result.status === "ok") {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      {label && (
        <span className="flex items-center gap-1.5 text-xs text-ink-2">
          <Volume2
            className="w-[13px] h-[13px] text-background-ui"
            aria-hidden="true"
          />
          {label}
        </span>
      )}
      <p className="m-0 text-[15px] leading-[23px] whitespace-pre-wrap break-words">
        {spoken}
        {live && detail === null && (
          <span
            aria-hidden="true"
            className="inline-block w-2 h-4 ms-0.5 align-text-bottom rounded-sm bg-background-ui animate-pulse"
          />
        )}
      </p>
      {detail && (
        <div className="border-t border-line pt-2 text-[13px] leading-[19px] break-words [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-wash [&_pre]:p-2 [&_code]:font-mono [&_code]:text-xs">
          <MarkdownContent markdown={detail} />
        </div>
      )}
      <div className="flex items-center gap-0.5 relative">
        <span className="flex-1 text-xs text-ink-2">{meta}</span>
        {!live && (
          <>
            <button
              type="button"
              onClick={() => commands.replayAskAnswer(thread.id, answer.id)}
              aria-label={t("ask.panel.replay")}
              title={t("ask.panel.replay")}
              className={smallIconButton}
            >
              <RotateCcw className="w-[15px] h-[15px]" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={copy}
              aria-label={copied ? t("ask.panel.copied") : t("ask.panel.copy")}
              title={copied ? t("ask.panel.copied") : t("ask.panel.copy")}
              className={smallIconButton}
            >
              {copied ? (
                <Check className="w-[15px] h-[15px]" aria-hidden="true" />
              ) : (
                <Copy className="w-[15px] h-[15px]" aria-hidden="true" />
              )}
            </button>
            <button
              type="button"
              onClick={() => setMoving((m) => !m)}
              aria-label={t("ask.panel.move")}
              title={t("ask.panel.move")}
              aria-expanded={moving}
              className={smallIconButton}
            >
              <MessageSquareShare
                className="w-[15px] h-[15px]"
                aria-hidden="true"
              />
            </button>
          </>
        )}
        {moving && (
          <div className="absolute right-0 bottom-9 z-10 w-60 rounded-xl border border-line bg-background shadow-lg p-1.5 flex flex-col gap-0.5 animate-drop">
            <span className="px-2 pt-1 pb-1 text-xs text-ink-2">
              {t("ask.panel.moveTo")}
            </span>
            {threads
              .filter((item) => item.id !== thread.id)
              .map((item) => (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => {
                    setMoving(false);
                    commands.moveAskQuestion(thread.id, questionId, item.id);
                  }}
                  className="px-2 py-1.5 rounded-lg text-start truncate hover:bg-wash cursor-pointer"
                >
                  {item.title}
                </button>
              ))}
            <button
              type="button"
              onClick={() => {
                setMoving(false);
                commands.moveAskQuestion(thread.id, questionId, null);
              }}
              className="px-2 py-1.5 rounded-lg text-start hover:bg-wash cursor-pointer flex items-center gap-1.5"
            >
              <Plus className="w-3.5 h-3.5" aria-hidden="true" />
              {t("ask.panel.moveNew")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
};
