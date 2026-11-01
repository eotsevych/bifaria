import { useEffect, useState } from "react";
import type { Tokens } from "@/lib/tokens";
import { Section, Row } from "./SettingsPrimitives";
import {
  fetchUpdateStatus,
  subscribeToUpdateStatus,
  triggerCheckForUpdates,
  triggerInstallUpdate,
  openWhatsNewWindow,
  fetchAppleTranslationInfo,
  type UpdateStatus,
} from "@/lib/tauri";

function statusText(status: UpdateStatus | null): string {
  if (!status) return "Updates are only available in the desktop app";
  if (status.store_managed) return "Updates come from the Microsoft Store";
  switch (status.state) {
    case "checking":
      return "Checking GitHub for a new version…";
    case "up_to_date":
      return `You're on the latest version (${status.current_version})`;
    case "available":
      return `Version ${status.version} is ready to install`;
    case "downloading":
      return status.percent != null
        ? `Downloading ${status.version}… ${status.percent}%`
        : `Downloading ${status.version}…`;
    case "installing":
      return `Installing ${status.version}. Bifaria will restart`;
    case "error":
      return status.message ? `Update failed: ${status.message}` : "Update failed";
    default:
      return "Bifaria checks for updates once a day";
  }
}

export default function UpdatesSection({ t }: { t: Tokens }) {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  // What's New is about Apple Translation, so it's offered only where that works.
  const [appleAvailable, setAppleAvailable] = useState(false);

  useEffect(() => {
    fetchAppleTranslationInfo()
      .then((info) => setAppleAvailable(info.available))
      .catch(() => setAppleAvailable(false));
  }, []);

  useEffect(() => {
    let active = true;
    fetchUpdateStatus().then((s) => {
      if (active) setStatus(s);
    });
    const unlisten = subscribeToUpdateStatus((s) => {
      setStatus(s);
      setInstallError(null);
    });
    return () => {
      active = false;
      unlisten.then((fn) => fn?.());
    };
  }, []);

  const busy =
    status?.state === "checking" ||
    status?.state === "downloading" ||
    status?.state === "installing";
  const available = status?.state === "available";

  const onCheck = async () => {
    setInstallError(null);
    const s = await triggerCheckForUpdates();
    if (s) setStatus(s);
  };

  const onInstall = async () => {
    setInstallError(null);
    const err = await triggerInstallUpdate();
    if (err) setInstallError(err);
  };

  const accentColor = available
    ? t.successColor
    : status?.state === "error"
      ? t.errorColor
      : t.textMuted;

  return (
    <Section title="Updates" t={t}>
      <Row
        label={
          <>
            {status ? `Bifaria ${status.current_version}` : "Bifaria"}
            <span
              style={{
                marginLeft: 8,
                padding: "1px 6px",
                borderRadius: 4,
                border: `1px solid ${t.accent}55`,
                background: `${t.accent}18`,
                color: t.accent,
                fontSize: 10,
                fontWeight: 600,
                letterSpacing: 0.4,
                textTransform: "uppercase",
                verticalAlign: "middle",
              }}
            >
              Beta
            </span>
          </>
        }
        hint={installError ?? statusText(status)}
        t={t}
        last={!appleAvailable && status?.state !== "downloading"}
      >
        {!status?.store_managed && (
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            {available && (
              <button
                type="button"
                onClick={onInstall}
                style={{
                  padding: "5px 10px",
                  borderRadius: 6,
                  border: `1px solid ${t.successColor}`,
                  background: `${t.successColor}20`,
                  color: t.successColor,
                  fontSize: 11,
                  cursor: "pointer",
                  fontWeight: 600,
                  whiteSpace: "nowrap",
                }}
              >
                Install & Restart
              </button>
            )}
            <button
              type="button"
              onClick={onCheck}
              disabled={!status || busy}
              style={{
                padding: "5px 10px",
                borderRadius: 6,
                border: `1px solid ${t.border}`,
                background: t.surface,
                color: accentColor,
                fontSize: 11,
                cursor: !status || busy ? "default" : "pointer",
                opacity: !status || busy ? 0.6 : 1,
                whiteSpace: "nowrap",
              }}
            >
              {status?.state === "checking" ? "Checking…" : "Check for Updates"}
            </button>
          </div>
        )}
      </Row>
      {status?.state === "downloading" && (
        <div style={{ padding: "0 16px 12px" }}>
          <div style={{ height: 4, borderRadius: 2, background: t.border, overflow: "hidden" }}>
            <div
              style={{
                height: "100%",
                width: `${status.percent ?? 100}%`,
                background: t.successColor,
                transition: "width 0.2s",
                opacity: status.percent == null ? 0.5 : 1,
              }}
            />
          </div>
        </div>
      )}
      {appleAvailable && (
        <Row label="What's New" hint="On-device translation with Apple Translation" t={t} last>
          <button
            type="button"
            onClick={() => openWhatsNewWindow()}
            style={{
              padding: "5px 10px",
              borderRadius: 6,
              border: `1px solid ${t.border}`,
              background: t.surface,
              color: t.accent,
              fontSize: 11,
              cursor: "pointer",
              fontWeight: 500,
              whiteSpace: "nowrap",
            }}
          >
            View
          </button>
        </Row>
      )}
    </Section>
  );
}
