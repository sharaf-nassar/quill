import { useState, type FormEvent } from "react";
import type { WebUiConfig } from "../../web/httpTransport";
import type { WebUiError } from "../../hooks/useWebUiSettings";

interface AllowlistEditorProps {
  id: string;
  /** Names the radio option this editor configures, for assistive tech. */
  labelledBy: string;
  config: WebUiConfig;
  disabled: boolean;
  save: (next: WebUiConfig) => Promise<WebUiError | null>;
}

function AllowlistEditor({
  id,
  labelledBy,
  config,
  disabled,
  save,
}: AllowlistEditorProps) {
  const [candidate, setCandidate] = useState("");
  const [entryError, setEntryError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const busy = disabled || submitting;

  const reportEntryError = (error: WebUiError | null) => {
    if (
      error?.code === "invalid_allowlist_entry" ||
      error?.code === "too_many_allowlist_entries"
    ) {
      setEntryError(error.message);
    }
  };

  const addEntry = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setEntryError(null);
    setSubmitting(true);
    const error = await save({
      ...config,
      allowlist: [...config.allowlist, candidate],
    });
    if (error === null) setCandidate("");
    else reportEntryError(error);
    setSubmitting(false);
  };

  const removeEntry = async (entry: string) => {
    setEntryError(null);
    setSubmitting(true);
    reportEntryError(
      await save({
        ...config,
        allowlist: config.allowlist.filter((candidate) => candidate !== entry),
      }),
    );
    setSubmitting(false);
  };

  return (
    <section id={id} className="web-allowlist" aria-labelledby={labelledBy}>
      <p className="web-allowlist-description">
        The names and addresses another device may use to reach this Quill —
        the host part of the URL you would type, not the device doing the
        typing. Listing anything beyond this machine also serves the listener
        on your network. Pairing still decides who gets in.
      </p>
      <form className="web-allowlist-form" onSubmit={addEntry}>
        <label className="web-allowlist-input-label" htmlFor="web-ui-allowlist-entry">
          Add address
        </label>
        <div className="web-allowlist-input-row">
          <input
            id="web-ui-allowlist-entry"
            className="settings-input web-allowlist-input"
            type="text"
            value={candidate}
            disabled={busy}
            spellCheck={false}
            placeholder="hostname or IP address"
            aria-invalid={entryError !== null}
            aria-describedby={entryError === null ? undefined : "web-allowlist-error"}
            onChange={(event) => {
              setCandidate(event.target.value);
              setEntryError(null);
            }}
          />
          <button type="submit" className="settings-button" disabled={busy}>
            Add
          </button>
        </div>
      </form>
      {entryError !== null && (
        <p id="web-allowlist-error" className="web-allowlist-error" role="alert">
          {entryError}
        </p>
      )}
      {config.allowlist.length === 0 ? (
        <p className="web-allowlist-empty">
          Nothing listed — this machine only, on loopback.
        </p>
      ) : (
        <ul className="web-allowlist-entries" aria-label="Allowed hosts">
          {config.allowlist.map((entry) => (
            <li key={entry} className="web-allowlist-entry">
              <code>{entry}</code>
              <button
                type="button"
                className="settings-button"
                disabled={busy}
                aria-label={`Stop answering to ${entry}`}
                onClick={() => void removeEntry(entry)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export default AllowlistEditor;
