import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import DOMPurify from "dompurify";
import { marked } from "marked";

interface ReleaseNote {
  tag_name: string;
  name: string | null;
  body: string | null;
  html_url: string;
  published_at: string | null;
}

type FetchState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; releases: ReleaseNote[] };

function formatDate(value: string | null): string {
  if (!value) return "";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.valueOf())) return "";
  return parsed.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function renderReleaseMarkdown(value: string): string {
  const markdown = value.replace(
    /^(?:\u200B|\u200C|\u200D|\u200E|\u200F|\uFEFF)+/u,
    "",
  );
  const html = marked.parse(markdown, {
    async: false,
    breaks: false,
    gfm: true,
  });

  return DOMPurify.sanitize(typeof html === "string" ? html : "", {
    FORBID_ATTR: ["style"],
    FORBID_TAGS: ["style"],
    USE_PROFILES: { html: true },
  });
}

function ReleaseNotesWindow() {
  const [state, setState] = useState<FetchState>({ status: "loading" });
  const [index, setIndex] = useState(0);

  const loadReleases = useCallback(async () => {
    setState({ status: "loading" });
    try {
      const releases = await invoke<ReleaseNote[]>("get_release_notes", {
        limit: 30,
      });
      setIndex(0);
      setState({ status: "ready", releases });
    } catch (err) {
      setState({ status: "error", message: String(err) });
    }
  }, []);

  useEffect(() => {
    void loadReleases();
  }, [loadReleases]);

  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
  }, []);

  const current = useMemo<ReleaseNote | null>(() => {
    if (state.status !== "ready") return null;
    return state.releases[index] ?? null;
  }, [state, index]);

  const releaseHtml = useMemo(() => {
    if (!current?.body) return "";
    return renderReleaseMarkdown(current.body);
  }, [current?.body]);

  const total = state.status === "ready" ? state.releases.length : 0;
  const canPrev = state.status === "ready" && index < total - 1;
  const canNext = state.status === "ready" && index > 0;

  const handlePrev = useCallback(() => {
    if (canPrev) setIndex((i) => i + 1);
  }, [canPrev]);

  const handleNext = useCallback(() => {
    if (canNext) setIndex((i) => i - 1);
  }, [canNext]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey
      ) {
        return;
      }

      if (event.key === "Escape") {
        event.preventDefault();
        void handleClose();
        return;
      }

      if (event.key === "ArrowLeft" && canPrev) {
        event.preventDefault();
        handlePrev();
        return;
      }

      if (event.key === "ArrowRight" && canNext) {
        event.preventDefault();
        handleNext();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canNext, canPrev, handleClose, handleNext, handlePrev]);

  return (
    <div className="release-notes-window">
      <div className="release-notes-titlebar" data-tauri-drag-region>
        <span className="release-notes-title" data-tauri-drag-region>
          Release Notes
        </span>
        <button
          className="release-notes-close"
          onClick={() => void handleClose()}
          aria-label="Close"
        >
          &times;
        </button>
      </div>

      {state.status === "ready" && state.releases.length > 0 && (
        <nav className="release-notes-toolbar" aria-label="Release navigation">
          <div className="release-notes-toolbar-main">
            <div className="release-notes-nav">
              <button
                className="release-notes-btn"
                onClick={handlePrev}
                disabled={!canPrev}
                aria-label="Previous release"
                title="Previous release"
              >
                ‹ Previous
              </button>
              <span
                className="release-notes-counter"
                aria-label={`Release ${index + 1} of ${total}`}
              >
                {index + 1} / {total}
              </span>
              <button
                className="release-notes-btn"
                onClick={handleNext}
                disabled={!canNext}
                aria-label="Next release"
                title="Next release"
              >
                Next ›
              </button>
            </div>
          </div>
          {current && (
            <span className="release-notes-source" title={current.html_url}>
              {current.html_url}
            </span>
          )}
        </nav>
      )}

      <div className="release-notes-body">
        {state.status === "loading" && (
          <div className="release-notes-status">Loading releases…</div>
        )}

        {state.status === "error" && (
          <div className="release-notes-status release-notes-status--error">
            <p className="release-notes-status-message">
              Could not load release notes.
            </p>
            <p className="release-notes-status-detail">{state.message}</p>
            <button
              className="release-notes-btn"
              onClick={() => void loadReleases()}
            >
              Retry
            </button>
          </div>
        )}

        {state.status === "ready" && state.releases.length === 0 && (
          <div className="release-notes-status">No releases available.</div>
        )}

        {state.status === "ready" && current && (
          <article className="release-notes-content" aria-live="polite">
            <header className="release-notes-content-header">
              <span className="release-notes-kicker">
                Release {index + 1} of {total}
              </span>
              <h2 className="release-notes-version">
                <span className="release-notes-version-tag">
                  {current.tag_name}
                </span>
              </h2>
              {current.published_at && (
                <div
                  className="release-notes-meta"
                  aria-label="Release metadata"
                >
                  <span className="release-notes-date">
                    {formatDate(current.published_at)}
                  </span>
                </div>
              )}
            </header>
            <div className="release-notes-scroll">
              {releaseHtml ? (
                <div
                  className="release-notes-markdown"
                  dangerouslySetInnerHTML={{ __html: releaseHtml }}
                />
              ) : (
                <p className="release-notes-empty-body">
                  No description provided for this release.
                </p>
              )}
            </div>
          </article>
        )}
      </div>
    </div>
  );
}

export default ReleaseNotesWindow;
