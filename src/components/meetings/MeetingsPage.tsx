import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useOsType } from "@/hooks/useOsType";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { useMeetingsStore } from "@/stores/meetingsStore";
import { MeetingDetail } from "./MeetingDetail";
import { MeetingList } from "./MeetingList";
import { MeetingSettingsPage } from "./MeetingSettingsPage";
import { StartMeetingDialog } from "./StartMeetingDialog";

type View =
  | { kind: "list" }
  | { kind: "detail"; id: string }
  | { kind: "settings" };

export const MeetingsPage: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const osType = useOsType();
  const {
    meetings,
    loaded,
    loadFailed,
    query,
    recording,
    initialize,
    setQuery,
  } = useMeetingsStore();
  const [view, setView] = useState<View>({ kind: "list" });
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    initialize();
  }, [initialize]);

  const shortcut = formatKeyCombination(
    settings?.bindings?.meeting_toggle?.current_binding ?? "ctrl+alt+m",
    osType,
  );

  const start = async () => {
    setStarting(false);
    const result = await commands.startMeeting();
    if (result.status === "error") {
      toast.error(t("meetings.startFailed", { error: result.error }));
    }
  };

  const stop = async () => {
    const result = await commands.stopMeeting();
    if (result.status === "error") {
      toast.error(t("meetings.stopFailed", { error: result.error }));
    }
  };

  const retry = async (id: string) => {
    const result = await commands.retryMeeting(id);
    if (result.status === "error") toast.error(result.error);
  };

  const discard = async (id: string) => {
    const result = await commands.deleteMeeting(id);
    if (result.status === "error") toast.error(result.error);
  };

  if (view.kind === "detail") {
    return (
      <MeetingDetail id={view.id} onBack={() => setView({ kind: "list" })} />
    );
  }
  if (view.kind === "settings") {
    return <MeetingSettingsPage onBack={() => setView({ kind: "list" })} />;
  }
  return (
    <>
      <MeetingList
        meetings={meetings}
        loaded={loaded}
        loadFailed={loadFailed}
        query={query}
        recording={recording}
        shortcut={shortcut}
        onQuery={setQuery}
        onStart={() => setStarting(true)}
        onStop={stop}
        onBookmark={() => commands.addMeetingBookmark()}
        onOpen={(id) => setView({ kind: "detail", id })}
        onSettings={() => setView({ kind: "settings" })}
        onRetry={retry}
        onDiscard={discard}
      />
      <StartMeetingDialog
        open={starting}
        onOpenChange={setStarting}
        onStart={start}
      />
    </>
  );
};
