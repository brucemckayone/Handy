import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Copy, Headphones } from "lucide-react";
import { commands, type MeetingOutputInfo } from "@/bindings";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";

interface StartMeetingDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onStart: () => void;
}

/** Asks the user to tell people they're recording, and warns when the call
 * plays through speakers the mic will hear. */
export const StartMeetingDialog: React.FC<StartMeetingDialogProps> = ({
  open,
  onOpenChange,
  onStart,
}) => {
  const { t } = useTranslation();
  const [output, setOutput] = useState<MeetingOutputInfo | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!open) return;
    setCopied(false);
    commands.getMeetingOutputInfo().then(setOutput);
  }, [open]);

  const copyConsent = async () => {
    const result = await commands.copyMeetingText(
      t("meetings.startDialog.consentMessage"),
    );
    if (result.status === "ok") setCopied(true);
  };

  const speakers = output?.is_speakers ?? false;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      closeLabel={t("meetings.close")}
      title={
        speakers
          ? t("meetings.startDialog.speakersTitle")
          : t("meetings.startDialog.title")
      }
      description={
        speakers
          ? t("meetings.startDialog.speakersBody", {
              device: output?.name ?? "",
            })
          : undefined
      }
      footer={
        <div className="flex justify-end gap-2 flex-wrap">
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("meetings.cancel")}
          </Button>
          {speakers ? (
            <>
              <Button variant="secondary" onClick={onStart}>
                {t("meetings.startDialog.recordAnyway")}
              </Button>
              <Button onClick={onStart}>
                <span className="inline-flex items-center gap-1.5">
                  <Headphones className="w-4 h-4" aria-hidden="true" />
                  {t("meetings.startDialog.headphonesOn")}
                </span>
              </Button>
            </>
          ) : (
            <Button onClick={onStart}>
              {t("meetings.startDialog.record")}
            </Button>
          )}
        </div>
      }
    >
      <div className="flex items-center gap-2 px-3 py-2 rounded-md bg-wash text-sm">
        <span className="flex-1">{t("meetings.startDialog.consent")}</span>
        <Button variant="secondary" size="sm" onClick={copyConsent}>
          <span className="inline-flex items-center gap-1">
            <Copy className="w-3 h-3" aria-hidden="true" />
            {copied
              ? t("meetings.startDialog.copied")
              : t("meetings.startDialog.copyMessage")}
          </span>
        </Button>
      </div>
    </Dialog>
  );
};
