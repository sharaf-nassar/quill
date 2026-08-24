import { useEffect, useState } from "react";
import { useWebUiSettings } from "../../hooks/useWebUiSettings";
import type { WebUiHostPolicy } from "../../web/httpTransport";
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

/** The controller binds exactly one of these two addresses. */
const LOOPBACK_BIND_PREFIX = "127.";

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
  const [portDraft, setPortDraft] = useState(String(config.port));
  const [portError, setPortError] = useState<string | null>(null);

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

  const setHostPolicy = (host_policy: WebUiHostPolicy) => {
    if (host_policy === config.host_policy) return;
    void save({ ...config, host_policy });
  };

  // `bound_addr` is the socket Quill holds on this machine. Whether a packet
  // from another device arrives at it is a firewall and routing question no
  // same-host check can answer, so the readout never claims it.
  const loopbackOnly =
    status.bound_addr !== null && status.bound_addr.startsWith(LOOPBACK_BIND_PREFIX);
  const lastError = error?.message ?? status.last_error;

  return (
    <div className="settings-panel">
      <div className="settings-section-header">Web UI</div>
      <div className="settings-prose">
        <p>
          Quill can serve its monitor view — the widget's Limits band and its
          data views — over plain HTTP to a browser on another device. Settings,
          Manage, Sessions, Learning, and Memory are never served, and a browser
          cannot change anything.
        </p>
        <p>
          <strong>A paired browser reads all of this, unredacted:</strong>{" "}
          rate-limit utilization and reset windows per provider and account;
          token, cost, and code-line totals over time; every project name and
          absolute project path; the hostnames Quill has recorded; session
          identifiers and live agent lineage; model IDs; skill and hook names;
          runtime statistics; context-savings figures; which providers are
          enabled; and your retention window.
        </p>
        <p>
          The connection is HTTP, not HTTPS: on a network you do not control,
          treat it as readable in transit. Pairing is always required, and until
          you allow a non-local host the listener binds loopback only.
        </p>
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
        <legend className="web-policy-legend">Which hosts may connect</legend>
        <label className="web-policy-option">
          <input
            type="radio"
            name="web-ui-host-policy"
            value="allowlist"
            checked={config.host_policy === "allowlist"}
            onChange={() => setHostPolicy("allowlist")}
          />
          <span className="web-policy-body">
            <span className="web-policy-label">Allowlist</span>
            <span className="web-policy-description">
              Only the addresses you list may connect; every other peer is
              refused before any data is read. An empty list allows this machine
              only, and the listener stays on loopback.
            </span>
          </span>
        </label>
        <label className="web-policy-option">
          <input
            type="radio"
            name="web-ui-host-policy"
            value="all"
            checked={config.host_policy === "all"}
            onChange={() => setHostPolicy("all")}
          />
          <span className="web-policy-body">
            <span className="web-policy-label">Accept all hosts</span>
            <span className="web-policy-description">
              The listener binds every network interface and any device that can
              route to the port may attempt to pair. Everything listed above
              becomes readable to whoever holds the pairing code — including
              over a VPN, a bridged interface, or public Wi-Fi.
            </span>
          </span>
        </label>
      </fieldset>

      <div className="settings-section-header">Pairing</div>
      <SettingRow
        label="Pairing code"
        description="A browser enters this code once and holds a session afterwards. Regenerating it signs out every paired browser immediately. This code is Quill's own credential — it is never the token your agents use to report usage."
        control={
          <>
            <code className="web-pairing-code">
              {loading ? "…" : pairingCode || "unavailable"}
            </code>
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
      {status.running ? (
        <div className="settings-prose">
          <p>
            {loopbackOnly
              ? "Bound on loopback, so only this machine can reach it. Add a non-local allowlist entry, or choose Accept all hosts, to bind the other network interfaces."
              : "Bound on every network interface. Quill can confirm it holds the socket on this machine and nothing more — a firewall here or anywhere on the route can still refuse another device, and Quill does not test reachability from one."}
          </p>
          <p>
            Open{" "}
            {status.reachable_urls.length === 0
              ? "—"
              : status.reachable_urls.map((url, index) => (
                  <span key={url}>
                    {index > 0 && ", "}
                    <code>{url}</code>
                  </span>
                ))}
          </p>
        </div>
      ) : (
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
