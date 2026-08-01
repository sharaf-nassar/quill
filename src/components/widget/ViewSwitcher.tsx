// ViewSwitcher — the widget's view dropdown.
//
// The 360px surface cannot host a tab bar for five views, so the view region's
// header carries its own name as a listbox trigger: the label *is* the current
// view, and opening it swaps everything below LIMITS.
//
// Listbox semantics rather than a menu, because the control has a value: the
// button is `aria-haspopup="listbox"`, the popup is `role="listbox"` with one
// `aria-selected` option, and keyboard movement runs through
// `aria-activedescendant` so the roving focus never leaves the list. Escape and
// an outside click both close and return focus to the trigger.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useCallback, useEffect, useId, useRef, useState } from "react";

/** Every view the widget's view region can host. */
export type WidgetView = "usage" | "trends" | "charts" | "models" | "context";

export interface ViewOption {
  readonly id: WidgetView;
  /** Sentence-case name shown on the trigger and in the list. */
  readonly label: string;
}

export interface ViewSwitcherProps {
  /**
   * The views that actually exist. Only implemented views are listed — a
   * dropdown entry that swaps to nothing would be a dead control.
   */
  options: readonly ViewOption[];
  view: WidgetView;
  onSelect: (view: WidgetView) => void;
}

const ChevronIcon = () => (
  <svg
    className="wg-viewdd-chev"
    width="8"
    height="8"
    viewBox="0 0 8 8"
    fill="none"
    stroke="currentColor"
    strokeWidth={1.3}
    strokeLinecap="round"
    aria-hidden="true"
    focusable={false}
  >
    <path d="M1.5 3l2.5 2.5L6.5 3" />
  </svg>
);

function ViewSwitcher({ options, view, onSelect }: ViewSwitcherProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listboxId = `${useId().replace(/\W/g, "")}-views`;

  const selectedIndex = Math.max(
    0,
    options.findIndex((option) => option.id === view),
  );
  const current = options[selectedIndex] ?? options[0];

  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }, []);

  // Opening starts the active option on the current value, so Enter without
  // any movement is a no-op rather than a silent jump to the first view.
  const openList = useCallback(() => {
    setActiveIndex(selectedIndex);
    setOpen(true);
  }, [selectedIndex]);

  useEffect(() => {
    if (!open) return;
    listRef.current?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (rootRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open]);

  const commit = useCallback(
    (index: number) => {
      const option = options[index];
      if (option) onSelect(option.id);
      close(true);
    },
    [close, onSelect, options],
  );

  const handleListKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if (event.key === "Tab") {
      close(false);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      commit(activeIndex);
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((index) => Math.min(index + 1, options.length - 1));
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((index) => Math.max(index - 1, 0));
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      setActiveIndex(0);
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      setActiveIndex(options.length - 1);
    }
  };

  return (
    <div className="wg-viewdd" data-open={open ? "true" : undefined} ref={rootRef}>
      <button
        type="button"
        className="wg-viewdd-btn"
        ref={buttonRef}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        onClick={() => (open ? close(false) : openList())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" && !open) {
            event.preventDefault();
            openList();
          }
        }}
      >
        {current?.label ?? "View"}
        <ChevronIcon />
      </button>
      {open && (
        <div
          className="wg-viewdd-menu"
          id={listboxId}
          role="listbox"
          aria-label="Widget view"
          aria-activedescendant={`${listboxId}-${activeIndex}`}
          tabIndex={-1}
          ref={listRef}
          onKeyDown={handleListKeyDown}
        >
          {options.map((option, index) => (
            <div
              key={option.id}
              id={`${listboxId}-${index}`}
              role="option"
              className="wg-viewdd-option"
              aria-selected={option.id === view}
              data-active={index === activeIndex ? "true" : undefined}
              onMouseEnter={() => setActiveIndex(index)}
              onClick={() => commit(index)}
            >
              {option.label}
              <span className="wg-viewdd-check" aria-hidden="true">
                ✓
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default ViewSwitcher;
