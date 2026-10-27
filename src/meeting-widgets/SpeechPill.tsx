import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Square, Volume2 } from "lucide-react";
import { commands, events, type SpeakRefusal } from "@/bindings";

const BARS = [0.45, 0.8, 1, 0.7, 0.4];

/** The pill shown while Handy reads aloud, with stop; and briefly for a
 * notice when something couldn't be read. */
export const SpeechPill: React.FC = () => {
  const { t } = useTranslation();
  const [speaking, setSpeaking] = useState(false);
  const [refusal, setRefusal] = useState<SpeakRefusal | null>(null);
  const noticeTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    commands.isSpeaking().then(setSpeaking);
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
    ];
    return () => {
      clearTimeout(noticeTimer.current);
      unlisteners.forEach((u) => u.then((fn) => fn()));
    };
  }, []);

  if (refusal && !speaking) {
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
  if (!speaking) return null;

  return (
    <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
      <div
        role="toolbar"
        aria-label={t("speech.pill.label")}
        className="h-[46px] flex items-center gap-3 ps-4 pe-1.5 rounded-full border border-line bg-background text-sm text-text shadow-md"
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
        <span>{t("speech.pill.reading")}</span>
        <button
          type="button"
          onClick={() => commands.stopSpeaking()}
          aria-label={t("speech.pill.stop")}
          title={t("speech.pill.stop")}
          className="w-8 h-8 flex items-center justify-center rounded-full bg-wash text-ink-2 hover:bg-logo-primary/30 hover:scale-105 transition-transform cursor-pointer"
        >
          <Square className="w-3.5 h-3.5 fill-current" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
};
