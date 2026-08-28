import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  PairingCodeResponse,
  WebUiConfig,
  WebUiConfigResponse,
  WebUiStatus,
} from "../web/httpTransport";

/**
 * Desktop-only read/write access to the `web_ui.*` configuration, the pairing
 * credential, and the live listener status (feature 029).
 *
 * The four commands this hook drives are deliberately absent from the web
 * transport's permitted-command table, so this surface exists on the desktop
 * and nowhere else. Config and status are separate reads because they answer
 * separate questions: the config is what the user asked for, the status is what
 * the listener actually did with it.
 */

/**
 * The `WebUiErrorCode` variants of `src-tauri/src/web_server/mod.rs`, plus
 * `unexpected` for a rejection that did not come from that typed boundary at
 * all (a panic or a serialization failure). Kept as a runtime list so a
 * rejection is recognized rather than cast.
 */
const WEB_UI_ERROR_CODES = [
  "pairing_unavailable",
  "invalid_port",
  "port_collision",
  "invalid_allowlist_entry",
  "too_many_allowlist_entries",
  "invalid_stored_configuration",
  "storage",
  "bind_failed",
  "rollback_failed",
] as const;

export type WebUiErrorCode = (typeof WEB_UI_ERROR_CODES)[number] | "unexpected";

/** The typed, display-safe failure the four Web UI commands reject with. */
export interface WebUiError {
  code: WebUiErrorCode;
  message: string;
}

export const WEB_UI_CONFIG_DEFAULTS: WebUiConfig = {
  enabled: false,
  port: 19878,
  allowlist: [],
};

const NO_LISTENER: WebUiStatus = {
  running: false,
  bound_addr: null,
  reachable_urls: [],
  last_error: null,
};

export interface UseWebUiSettingsResult {
  config: WebUiConfig;
  /** The short display encoding of the pairing credential. */
  pairingCode: string;
  status: WebUiStatus;
  loading: boolean;
  saving: boolean;
  /**
   * The last rejection. The backend preserves the last-known-good
   * configuration on every failed transition, so `config` still holds what is
   * persisted and running.
   */
  error: WebUiError | null;
  save: (next: WebUiConfig) => Promise<WebUiError | null>;
  regeneratePairingCode: () => Promise<void>;
}

function asWebUiError(rejection: unknown): WebUiError {
  if (
    typeof rejection === "object" &&
    rejection !== null &&
    "code" in rejection &&
    "message" in rejection &&
    typeof rejection.message === "string" &&
    WEB_UI_ERROR_CODES.some((code) => code === rejection.code)
  ) {
    return { code: rejection.code as WebUiErrorCode, message: rejection.message };
  }
  return { code: "unexpected", message: String(rejection) };
}

export function useWebUiSettings(): UseWebUiSettingsResult {
  const [config, setConfig] = useState<WebUiConfig>(WEB_UI_CONFIG_DEFAULTS);
  const [pairingCode, setPairingCode] = useState("");
  const [status, setStatus] = useState<WebUiStatus>(NO_LISTENER);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<WebUiError | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    let failure: WebUiError | null = null;
    try {
      const response = await invoke<WebUiConfigResponse>("get_web_ui_config");
      setConfig(response.config);
      setPairingCode(response.pairing_code);
    } catch (e) {
      failure = asWebUiError(e);
    }
    try {
      setStatus(await invoke<WebUiStatus>("get_web_ui_status"));
    } catch (e) {
      failure ??= asWebUiError(e);
    }
    setError(failure);
    setLoading(false);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = useCallback(async (next: WebUiConfig) => {
    setSaving(true);
    let failure: WebUiError | null = null;
    try {
      // The backend returns the configuration it canonicalized and stored, so a
      // rejected candidate leaves this hook holding the running config rather
      // than an optimistic one the listener never accepted.
      const response = await invoke<WebUiConfigResponse>("set_web_ui_config", {
        config: next,
      });
      setConfig(response.config);
      setPairingCode(response.pairing_code);
    } catch (e) {
      failure = asWebUiError(e);
    }
    // Both outcomes can move the listener: a successful transition rebinds, and
    // a failed one records `last_error` while rolling back to the old socket.
    try {
      setStatus(await invoke<WebUiStatus>("get_web_ui_status"));
    } catch (e) {
      failure ??= asWebUiError(e);
    }
    setError(failure);
    setSaving(false);
    return failure;
  }, []);

  const regeneratePairingCode = useCallback(async () => {
    setSaving(true);
    try {
      const response = await invoke<PairingCodeResponse>(
        "regenerate_web_pairing_code",
      );
      setPairingCode(response.pairing_code);
      setError(null);
    } catch (e) {
      setError(asWebUiError(e));
    }
    setSaving(false);
  }, []);

  return {
    config,
    pairingCode,
    status,
    loading,
    saving,
    error,
    save,
    regeneratePairingCode,
  };
}
