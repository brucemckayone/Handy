import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  PanelRight,
  Square,
  TriangleAlert,
  Volume2,
} from "lucide-react";
import {
  commands,
  events,
  type AskBusyEvent,
  type SpeakRefusal,
} from "@/bindings";

const BARS = [0.45, 0.8, 1, 0.7, 0.4];
const NO_ASK: AskBusyEvent = { busy: false, thread_id: null, failed: false };

const pillClass =
  "h-[46px] flex items-center gap-3 ps-4 pe-1.5 rounded-full border border-line bg-background text-sm text-text shadow-md";
const roundButton =
  "w-8 h-8 flex items-center justify-center rounded-full bg-wash text-ink-2 hover:bg-logo-primary/30 hover:scale-105 transition-transform cursor-pointer";

/** The pill shown while Handy reads aloud, with stop; briefly for a notice
 * when something couldn't be read; and for Ask: thinking, the spoken
 * answer, then "Answered" with the button that opens the Ask panel. */
export const SpeechPill: React.FC = () => {
  const { t } = useTranslation();
  const [speaking, setSpeaking] = useState(false);
  const [refusal, setRefusal] = useState<SpeakRefusal | null>(null);
  const [ask, setAsk] = useState<AskBusyEvent>(NO_ASK);
  const noticeTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    commands.isSpeaking().then(setSpeaking);
    commands.getAskPillState().then(setAsk);
    const unlisteners = [
      events.speechStateEvent.listen(({ payload }) => {
        setSpeaking(payload.speaking);
        if (payload.speaking) setRefusal(null);
      }),
      events.speechRefusedEvent.listen(({ payload }) => {
        setRefusal(payload.refusal);
        clearTimeout(noticeTimer.current);
        noticeTimer.current = setTimeout(() => setRefusal(null), 3000);
      }),
      events.askBusyEvent.listen(({ payload }) => setAsk(payload)),
    ];
    return () => {
      clearTimeout(noticeTimer.current);
      unlisteners.forEach((u) => u.then((fn) => fn()));
    };
  }, []);

  const openButton = ask.thread_id && (
    <button
      type="button"
      onClick={() => commands.openAskPanel(ask.thread_id)}
      aria-label={t("ask.pill.openPanel")}
      title={t("ask.pill.openPanel")}
      className={roundButton}
    >
      <PanelRight className="w-4 h-4" aria-hidden="true" />
    </button>
  );

  if (refusal && !speaking && !ask.busy) {
    return (
      <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
        <div
          role="status"
          className="h-[46px] flex items-center px-5 rounded-full border border-line bg-background text-sm text-ink-2 shadow-md"
        >
          {t(`speech.refused.${refusal}`)}
        </div>
      </div>
    );
  }

  if (speaking) {
    return (
      <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
        <div
          role="toolbar"
          aria-label={t("speech.pill.label")}
          className={pillClass}
        >
          <Volume2 className="w-4 h-4 text-ink-2" aria-hidden="true" />
          <span aria-hidden="true" className="flex items-center gap-[3px] h-5">
            {BARS.map((weight, i) => (
              <span
                key={i}
                className="block w-1 rounded-sm bg-background-ui animate-pulse"
                style={{
                  height: `${Math.round(20 * weight)}px`,
                  animationDelay: `${i * 120}ms`,
                }}
              />
            ))}
          </span>
          <span>
            {ask.thread_id ? t("ask.pill.answering") : t("speech.pill.reading")}
          </span>
          <span className="flex items-center gap-1">
            {openButton}
            <button
              type="button"
              onClick={() => commands.stopSpeaking()}
              aria-label={t("speech.pill.stop")}
              title={t("speech.pill.stop")}
              className={roundButton}
            >
              <Square className="w-3.5 h-3.5 fill-current" aria-hidden="true" />
            </button>
          </span>
        </div>
      </div>
    );
  }

  if (ask.busy) {
    return (
      <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
        <div
          role="status"
          aria-label={t("ask.pill.label")}
          className={pillClass}
        >
          <span aria-hidden="true" className="flex items-center gap-1">
            {[0, 1, 2].map((i) => (
              <span
                key={i}
                className="block w-1.5 h-1.5 rounded-full bg-background-ui animate-pulse"
                style={{ animationDelay: `${i * 160}ms` }}
              />
            ))}
          </span>
          <span>{t("ask.pill.thinking")}</span>
          <span className="flex items-center gap-1">
            {openButton}
            <button
              type="button"
              onClick={() => commands.cancelAsk()}
              aria-label={t("ask.pill.cancel")}
              title={t("ask.pill.cancel")}
              className={roundButton}
            >
              <Square className="w-3.5 h-3.5 fill-current" aria-hidden="true" />
            </button>
          </span>
        </div>
      </div>
    );
  }

  if (ask.thread_id) {
    return (
      <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
        <div role="status" className={pillClass}>
          {ask.failed ? (
            <TriangleAlert
              className="w-4 h-4 text-warning"
              aria-hidden="true"
            />
          ) : (
            <Check className="w-4 h-4 text-moss" aria-hidden="true" />
          )}
          <span>
            {ask.failed ? t("ask.pill.failed") : t("ask.pill.answered")}
          </span>
          <button
            type="button"
            onClick={() => commands.openAskPanel(ask.thread_id)}
            className="h-8 px-3 flex items-center gap-1.5 rounded-full bg-wash text-ink-2 hover:bg-logo-primary/30 transition-colors cursor-pointer"
          >
            <PanelRight className="w-3.5 h-3.5" aria-hidden="true" />
            {t("ask.pill.open")}
          </button>
        </div>
      </div>
    );
  }

  return null;
};
