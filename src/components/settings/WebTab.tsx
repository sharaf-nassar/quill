import { useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { QRCodeSVG } from "qrcode.react";
import { useWebUiSettings } from "../../hooks/useWebUiSettings";
import { useToast } from "../../hooks/useToast";
import { handleExternalClick } from "../../lib/openExternal";
import AllowlistEditor from "./AllowlistEditor";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";

/**
 * The Web UI settings section (feature 029).
 *
 * P11 governs this surface: the browser listener is off by default, and the
 * disclosure below states what a paired browser can read *before* the toggle
 * can be turned on. The listed items are exactly the sixteen monitor reads the
 * server-side command allowlist permits — nothing is redacted, so the copy
 * names them rather than summarizing them away.
 */

const MIN_PORT = 1024;
const MAX_PORT = 65535;

/**
 * `web_config.rs` owns port validation, but a `u16` command argument cannot
 * carry a value that overflows it, so the field guards its own range before
 * invoking. Same range, same sentence, deliberately.
 */
const PORT_RANGE_MESSAGE = "Web UI port must be between 1024 and 65535.";


const ALLOWLIST_ID = "web-ui-allowlist";
const ALLOWLIST_LABEL_ID = "web-ui-allowlist-label";

/**
 * The one address this machine can always open, whichever interface the
 * listener bound. `Bound address` stays a plain socket fact — `0.0.0.0` is not
 * somewhere a browser goes, and a link labelled with it would navigate
 * somewhere its own text does not name. This is the link instead, and it says
 * exactly where it leads.
 *
 * The bare origin is the right target: the listener redirects an unpaired
 * browser to its pairing page itself, so no surface has to hand out a
 * bootstrap-only URL.
 */
function localhostUrl(boundAddr: string | null): string | null {
  if (boundAddr === null) return null;
  const port = boundAddr.slice(boundAddr.lastIndexOf(":") + 1);
  return /^\d+$/.test(port) ? `http://127.0.0.1:${port}/` : null;
}

/**
 * The allowlist entries a phone could put in a URL: everything the user listed
 * that names more than this machine. Loopback names are what the listener
 * answers to anyway, and a QR code pointing a phone at `127.0.0.1` would send
 * it to itself. Entries are canonical IPs or hostnames, so a bare IPv6 address
 * is the only form that needs brackets to become a URL host.
 */
function pairingUrls(allowlist: string[], port: number, code: string): string[] {
  return allowlist
    .filter((entry) => !/^(localhost|127\.|::1$)/i.test(entry))
    .map((entry) => (entry.includes(":") ? `[${entry}]` : entry))
    .map((host) => `http://${host}:${port}/pair#${code}`);
}

function WebTab() {
  const {
    config,
    pairingCode,
    status,
    loading,
    saving,
    error,
    save,
    regeneratePairingCode,
  } = useWebUiSettings();
  const { toast } = useToast();
  const [portDraft, setPortDraft] = useState(String(config.port));
  const [portError, setPortError] = useState<string | null>(null);

  // The code exists to be typed into another device, so clicking it should hand
  // it over. This goes through the OS clipboard rather than
  // `navigator.clipboard`, which fails silently under WebKitGTK on focus and
  // permission edge cases — and a copy that reports success while leaving the
  // clipboard stale is worse than no copy at all. A refusal is surfaced.
  const copyPairingCode = async () => {
    if (!pairingCode) return;
    try {
      await writeText(pairingCode);
      toast("info", "Pairing code copied");
    } catch (copyError) {
      toast("error", `Could not copy the pairing code: ${String(copyError)}`);
    }
  };

  // The field is the user's while they type; the persisted port takes it back
  // whenever the backend reports a different one (first load, or a save that
  // canonicalized the value).
  useEffect(() => {
    setPortDraft(String(config.port));
    setPortError(null);
  }, [config.port]);

  const busy = loading || saving;

  const commitPort = () => {
    const parsed = Number(portDraft);
    if (!Number.isInteger(parsed) || parsed < MIN_PORT || parsed > MAX_PORT) {
      setPortError(PORT_RANGE_MESSAGE);
      return;
    }
    setPortError(null);
    if (parsed === config.port) return;
    void save({ ...config, port: parsed });
  };

  // `bound_addr` is the socket Quill holds on this machine. Whether a packet
  // from another device arrives at it is a firewall and routing question no
  // same-host check can answer, so the readout states the socket and no more.
  const openUrl = status.running ? localhostUrl(status.bound_addr) : null;
  const lastError = error?.message ?? status.last_error;
  // The code rides in the URL fragment, which a browser never sends on the
  // wire; the pairing page submits it and strips it from history.
  const qrUrls =
    status.running && pairingCode
      ? pairingUrls(config.allowlist, config.port, pairingCode)
      : [];

  return (
    <div className="settings-panel">
      <div className="settings-section-header">Web UI</div>
      {/* P11 informed opt-in, at the length someone will actually read. The
          exhaustive field list this replaced was accurate and skipped; naming
          what is surprising — paths — discloses more in practice. */}
      <div className="settings-prose">
        <p>
          See your usage in a browser on another device. Read-only — a browser
          cannot change anything, and never sees Settings, Sessions, Learning,
          or Memory.
        </p>
        <p>
          <strong>A paired browser sees everything the widget shows</strong>,
          including your project names and their full paths on this machine.
        </p>
        <p>Traffic is unencrypted, so use it only on a network you trust.</p>
      </div>
      <SettingRow
        label="Serve the monitor view in a browser"
        description={
          config.enabled
            ? "The listener is on. Turning it off releases the port without restarting Quill."
            : "Off. No socket is bound on the configured port until you turn this on."
        }
        control={
          <Toggle
            tone={saving ? "busy" : config.enabled ? "on" : "off"}
            pressed={config.enabled}
            disabled={busy}
            ariaLabel="Serve the monitor view in a browser"
            onClick={() => void save({ ...config, enabled: !config.enabled })}
          />
        }
      />
      <SettingRow
        label="Port"
        description={
          portError ??
          "Range 1024–65535. Cannot be the Quill ingestion or context server port. Changing it while enabled rebinds and releases the old port."
        }
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            aria-label="Web UI port"
            aria-invalid={portError !== null}
            min={MIN_PORT}
            max={MAX_PORT}
            step={1}
            value={portDraft}
            disabled={busy}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={commitPort}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitPort();
            }}
          />
        }
      />

      <fieldset className="web-policy" disabled={busy}>
        <legend id={ALLOWLIST_LABEL_ID} className="web-policy-legend">
          Addresses this Quill answers to
        </legend>
        <AllowlistEditor
          id={ALLOWLIST_ID}
          labelledBy={ALLOWLIST_LABEL_ID}
          config={config}
          disabled={busy}
          save={save}
        />
      </fieldset>

      <div className="settings-section-header">Pairing</div>
      <SettingRow
        label="Pairing code"
        description="A browser enters this code once and holds a session afterwards. Regenerating it signs out every paired browser immediately. This code is Quill's own credential — it is never the token your agents use to report usage."
        control={
          <>
            <button
              type="button"
              className="web-pairing-code"
              disabled={loading || !pairingCode}
              onClick={() => void copyPairingCode()}
              title="Copy the pairing code"
              aria-label={
                pairingCode ? `Copy pairing code ${pairingCode}` : "Pairing code unavailable"
              }
            >
              {loading ? "…" : pairingCode || "unavailable"}
            </button>
            <button
              type="button"
              className="settings-button"
              disabled={busy}
              onClick={() => void regeneratePairingCode()}
            >
              Regenerate
            </button>
          </>
        }
      />
      <SettingRow
        label="Scan to pair"
        description={
          qrUrls.length > 0
            ? "Point your phone's camera at a code. It opens the pairing page and pairs on its own. With several codes, use the address your phone can reach."
            : status.running
              ? "Add a network address above to get a QR code a phone can scan."
              : "Turn the listener on to get a QR code a phone can scan."
        }
        control={
          qrUrls.length > 0 && (
            <div className="web-pairing-qr-list">
              {qrUrls.map((url) => {
                const label = url.slice("http://".length, url.indexOf("/pair"));
                return (
                  <figure key={url} className="web-pairing-qr">
                    <QRCodeSVG
                      value={url}
                      size={128}
                      marginSize={2}
                      aria-label={`Pairing QR code for ${label}`}
                    />
                    <figcaption>{label}</figcaption>
                  </figure>
                );
              })}
            </div>
          )
        }
      />

      <div className="settings-section-header">Listener</div>
      <dl className="web-status">
        <div className="web-status-item">
          <dt>State</dt>
          <dd>{status.running ? "Running" : "Stopped"}</dd>
        </div>
        <div className="web-status-item">
          <dt>Bound address</dt>
          <dd>{status.bound_addr ?? "—"}</dd>
        </div>
      </dl>
      {openUrl !== null && (
        <p className="web-open">
          <a
            className="settings-external-link"
            href={openUrl}
            onClick={handleExternalClick}
          >
            Open <code>{openUrl}</code>
          </a>
        </p>
      )}
      {!status.running && (
        <div className="settings-empty">
          No socket is bound. The configured port is connection-refused.
        </div>
      )}
      {lastError !== null && (
        <div className="settings-empty settings-empty--error" role="alert">
          Last error: {lastError}
        </div>
      )}
    </div>
  );
}

export default WebTab;
