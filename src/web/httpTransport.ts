// Web-only Tauri invoke shim.
//
// Wire names, statuses, command table, fixtures, and error strings derive from
// specs/029-web-ui-server.md#web-transport-protocol-contract.

export const WEB_INVOKE_PATH = "/api/web/invoke";
export const WEB_PAIR_PATH = "/api/web/pair";



export interface InvokeRequest {
  cmd: string;
  args: Record<string, unknown>;
}

export interface InvokeSuccess<T> {
  ok: true;
  value: T;
}

export interface InvokeCommandDenied {
  ok: false;
  code: "command_denied";
}

export interface InvokeCommandError {
  ok: false;
  code: "command_error";
  message: string;
}

export type InvokeResponse<T> =
  | InvokeSuccess<T>
  | InvokeCommandDenied
  | InvokeCommandError;

export interface PairRequest {
  code: string;
}

export interface WebUiConfig {
  enabled: boolean;
  port: number;
  allowlist: string[];
}

export interface WebUiConfigResponse {
  config: WebUiConfig;
  pairing_code: string;
}

export interface PairingCodeResponse {
  pairing_code: string;
}

export interface WebUiStatus {
  running: boolean;
  bound_addr: string | null;
  reachable_urls: string[];
  last_error: string | null;
}

interface InvokeFixture<T> {
  request: InvokeRequest;
  status: 200 | 403;
  response: InvokeResponse<T>;
}

interface PairFixture {
  request: PairRequest;
  success_status: 204;
}

export const WEB_PROTOCOL_FIXTURES = {
  success: {
    request: { cmd: "get_provider_statuses", args: {} },
    status: 200,
    response: { ok: true, value: [] },
  },
  commandDenied: {
    request: { cmd: "set_runtime_settings", args: { settings: {} } },
    status: 403,
    response: { ok: false, code: "command_denied" },
  },
  commandError: {
    request: {
      cmd: "get_model_usage_overview",
      args: { range: "24h", provider: null },
    },
    status: 200,
    response: {
      ok: false,
      code: "command_error",
      message: "Model analytics unavailable.",
    },
  },
  pairRequest: {
    request: { code: "fixture-pair-code" },
    success_status: 204,
  },
} as const satisfies {
  success: InvokeFixture<readonly unknown[]>;
  commandDenied: InvokeFixture<never>;
  commandError: InvokeFixture<never>;
  pairRequest: PairFixture;
};

export const WEB_TRANSPORT_ERRORS = {
  accessDenied: "Web UI access denied.",
  commandDenied: "Command is not available in the web UI.",
  invalidInvokeResponse: "Web UI returned an invalid invoke response.",
  unavailable: "Web UI is unavailable.",
  httpStatus: (status: number) => `Web UI request failed (HTTP ${status}).`,
} as const;

type FetchLike = typeof fetch;
type Callback = ((payload: unknown) => void) | undefined;

interface WebTauriInternals {
  invoke<T>(cmd: string, args?: unknown, options?: unknown): Promise<T>;
  transformCallback(callback?: Callback, once?: boolean): number;
  unregisterCallback(id: number): void;
  runCallback(id: number, payload: unknown): void;
  callbacks: Map<number, (payload: unknown) => void>;
  convertFileSrc(filePath: string): string;
  metadata: {
    currentWindow: { label: string };
    currentWebview: { windowLabel: string; label: string };
  };
}

interface WebTransportWindow {
  __TAURI_INTERNALS__?: Partial<WebTauriInternals>;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: {
    unregisterListener(event: string, eventId: number): void;
  };
  __QUILL_WEB_TRANSPORT__?: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function hasOwn(value: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every((key) => hasOwn(value, key));
}

function isInvokeResponse(value: unknown): value is InvokeResponse<unknown> {
  if (!isRecord(value) || typeof value.ok !== "boolean") return false;
  if (value.ok) return hasExactKeys(value, ["ok", "value"]);
  if (value.code === "command_denied") {
    return hasExactKeys(value, ["ok", "code"]);
  }
  return (
    value.code === "command_error" &&
    typeof value.message === "string" &&
    hasExactKeys(value, ["ok", "code", "message"])
  );
}

export async function decodeInvokeResponse<T>(response: Response): Promise<T> {
  if (response.status !== 200 && response.status !== 403) {
    return Promise.reject(WEB_TRANSPORT_ERRORS.httpStatus(response.status));
  }

  const body = await response.text();
  if (response.status === 403 && body.length === 0) {
    return Promise.reject(WEB_TRANSPORT_ERRORS.accessDenied);
  }

  let payload: unknown;
  try {
    payload = JSON.parse(body);
  } catch {
    return Promise.reject(WEB_TRANSPORT_ERRORS.invalidInvokeResponse);
  }

  if (!isInvokeResponse(payload)) {
    return Promise.reject(WEB_TRANSPORT_ERRORS.invalidInvokeResponse);
  }
  if (response.status === 200 && payload.ok) return payload.value as T;
  if (
    response.status === 200 &&
    !payload.ok &&
    payload.code === "command_error"
  ) {
    return Promise.reject(payload.message);
  }
  if (
    response.status === 403 &&
    !payload.ok &&
    payload.code === "command_denied"
  ) {
    return Promise.reject(WEB_TRANSPORT_ERRORS.commandDenied);
  }
  return Promise.reject(WEB_TRANSPORT_ERRORS.invalidInvokeResponse);
}

export async function invokeWebCommand<T>(
  cmd: string,
  args: unknown = {},
  fetchImpl: FetchLike = globalThis.fetch,
): Promise<T> {
  if (!isRecord(args)) {
    return Promise.reject("Web UI invoke arguments must be an object.");
  }

  let response: Response;
  try {
    response = await fetchImpl(WEB_INVOKE_PATH, {
      method: "POST",
      credentials: "same-origin",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ cmd, args } satisfies InvokeRequest),
    });
  } catch {
    return Promise.reject(WEB_TRANSPORT_ERRORS.unavailable);
  }

  return decodeInvokeResponse<T>(response);
}

export async function pairWebUi(
  code: string,
  fetchImpl: FetchLike = globalThis.fetch,
): Promise<boolean> {
  let response: Response;
  try {
    response = await fetchImpl(WEB_PAIR_PATH, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code } satisfies PairRequest),
    });
  } catch {
    return Promise.reject(WEB_TRANSPORT_ERRORS.unavailable);
  }

  if (response.status === 204) return true;
  if (response.status === 403 && (await response.text()).length === 0) {
    return false;
  }
  return Promise.reject(WEB_TRANSPORT_ERRORS.httpStatus(response.status));
}

function localInvokeResult(
  cmd: string,
  args: unknown,
  unregisterCallback: (id: number) => void,
): { handled: true; value: unknown } | { handled: false } {
  if (cmd === "plugin:event|listen") {
    const handler = isRecord(args) ? args.handler : null;
    return { handled: true, value: typeof handler === "number" ? handler : 0 };
  }
  if (cmd === "plugin:event|unlisten") {
    const eventId = isRecord(args) ? args.eventId : null;
    if (typeof eventId === "number") unregisterCallback(eventId);
    return { handled: true, value: null };
  }
  if (cmd.startsWith("plugin:window|") || cmd.startsWith("plugin:webview|")) {
    return { handled: true, value: null };
  }
  return { handled: false };
}

/** Install before importing any module that calls Tauri APIs. */
export function installHttpTransport(fetchImpl: FetchLike = globalThis.fetch): void {
  const target = window as unknown as WebTransportWindow;
  if (target.__QUILL_WEB_TRANSPORT__) return;

  const callbacks = new Map<number, (payload: unknown) => void>();
  let nextCallbackId = 1;
  const unregisterCallback = (id: number) => callbacks.delete(id);

  const internals: WebTauriInternals = {
    invoke: async <T>(cmd: string, args: unknown = {}): Promise<T> => {
      const local = localInvokeResult(cmd, args, unregisterCallback);
      if (local.handled) return local.value as T;
      return invokeWebCommand<T>(cmd, args, fetchImpl);
    },
    transformCallback: (callback, once = false) => {
      const id = nextCallbackId++;
      callbacks.set(id, (payload) => {
        if (once) callbacks.delete(id);
        callback?.(payload);
      });
      return id;
    },
    unregisterCallback,
    runCallback: (id, payload) => callbacks.get(id)?.(payload),
    callbacks,
    convertFileSrc: (filePath) => filePath,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
  };

  target.__TAURI_INTERNALS__ = internals;
  target.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (_event, eventId) => unregisterCallback(eventId),
  };
  target.__QUILL_WEB_TRANSPORT__ = true;
}
