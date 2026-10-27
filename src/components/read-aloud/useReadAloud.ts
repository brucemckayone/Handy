import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, events } from "@/bindings";

/** Whether Handy is reading aloud, and reading something in place of
 * whatever it's reading now. */
export const useReadAloud = () => {
  const { t } = useTranslation();
  const [speaking, setSpeaking] = useState(false);

  useEffect(() => {
    commands.isSpeaking().then(setSpeaking);
    const unlisten = events.speechStateEvent.listen(({ payload }) =>
      setSpeaking(payload.speaking),
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const read = useCallback(
    async (text: string) => {
      await commands.stopSpeaking();
      const result = await commands.speakText(text);
      if (result.status === "error") {
        toast(t(`speech.refused.${result.error}`));
      }
    },
    [t],
  );
  const stop = useCallback(() => commands.stopSpeaking(), []);

  return { speaking, read, stop };
};
