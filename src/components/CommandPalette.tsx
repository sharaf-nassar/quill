import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactElement } from "react";

export interface PaletteCommand {
  id: string;
  label: string;
  hint?: string;
  Icon?: () => ReactElement;
  run: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: PaletteCommand[];
  placeholder?: string;
}

// A minimal ⌘K palette: substring-filter a flat command list, arrow/enter to
// run. Controlled by the parent (open + commands); matches the ConfirmDialog
// modal conventions (backdrop mousedown-to-close, Escape, role="dialog").
function CommandPalette({
  open,
  onClose,
  commands,
  placeholder = "Search sections and actions…",
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter(
      (c) =>
        c.label.toLowerCase().includes(q) ||
        (c.hint?.toLowerCase().includes(q) ?? false),
    );
  }, [query, commands]);

  // Reset and focus the input each time the palette opens.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setIndex(0);
    const raf = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(raf);
  }, [open]);

  // Keep the selection in range as the filter narrows.
  useEffect(() => {
    setIndex((i) => (i >= filtered.length ? 0 : i));
  }, [filtered.length]);

  // Keep the active row visible while arrowing.
  useEffect(() => {
    if (!open) return;
    const row = listRef.current?.children[index] as HTMLElement | undefined;
    row?.scrollIntoView({ block: "nearest" });
  }, [index, open]);

  if (!open) return null;

  const run = (cmd: PaletteCommand | undefined) => {
    if (!cmd) return;
    onClose();
    cmd.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => (filtered.length ? (i + 1) % filtered.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) =>
        filtered.length ? (i - 1 + filtered.length) % filtered.length : 0,
      );
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(filtered[index]);
    }
  };

  return (
    <div className="cmdk-backdrop" onMouseDown={onClose}>
      <div
        className="cmdk"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <input
          ref={inputRef}
          className="cmdk-input"
          type="text"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          placeholder={placeholder}
          aria-label="Search commands"
          role="combobox"
          aria-expanded
          aria-controls="cmdk-list"
          autoComplete="off"
          spellCheck={false}
        />
        <ul className="cmdk-list" id="cmdk-list" role="listbox" ref={listRef}>
          {filtered.length === 0 ? (
            <li className="cmdk-empty">No matches</li>
          ) : (
            filtered.map((cmd, i) => {
              const Icon = cmd.Icon;
              return (
                <li key={cmd.id} role="option" aria-selected={i === index}>
                  <button
                    type="button"
                    className={`cmdk-item${i === index ? " active" : ""}`}
                    onMouseMove={() => setIndex(i)}
                    onClick={() => run(cmd)}
                  >
                    {Icon ? (
                      <span className="cmdk-item-icon">
                        <Icon />
                      </span>
                    ) : null}
                    <span className="cmdk-item-label">{cmd.label}</span>
                    {cmd.hint ? (
                      <span className="cmdk-item-hint">{cmd.hint}</span>
                    ) : null}
                  </button>
                </li>
              );
            })
          )}
        </ul>
      </div>
    </div>
  );
}

export default CommandPalette;
