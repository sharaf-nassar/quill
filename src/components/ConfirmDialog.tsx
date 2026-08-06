import type { ReactNode } from "react";
import { useEffect, useRef } from "react";

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
  busy?: boolean;
  confirmDisabled?: boolean;
  children?: ReactNode;
  onCancel: () => void;
  onConfirm: () => Promise<void> | void;
}

function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel = "Cancel",
  destructive = false,
  busy = false,
  confirmDisabled = false,
  children,
  onCancel,
  onConfirm,
}: ConfirmDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (open && dialog && !dialog.open) dialog.showModal();
  }, [open]);

  if (!open) return null;

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog"
      aria-label={title}
      onCancel={(event) => {
        // Escape while busy must not dismiss, matching the button guards.
        if (busy) event.preventDefault();
      }}
      onClose={onCancel}
      onMouseDown={(event) => {
        // Backdrop clicks dispatch to the dialog element itself; a click
        // inside the panel (including its padding) lands within its rect.
        const rect = event.currentTarget.getBoundingClientRect();
        const inside =
          event.clientX >= rect.left &&
          event.clientX <= rect.right &&
          event.clientY >= rect.top &&
          event.clientY <= rect.bottom;
        if (!inside && !busy) onCancel();
      }}
    >
      <h2 className="confirm-dialog-title">{title}</h2>
      <p className="confirm-dialog-description">{description}</p>
      {children}
      <div className="confirm-dialog-actions">
        <button className="confirm-dialog-btn" onClick={onCancel} disabled={busy}>
          {cancelLabel}
        </button>
        <button
          className={`confirm-dialog-btn confirm-dialog-btn--confirm${destructive ? " confirm-dialog-btn--destructive" : ""}`}
          onClick={onConfirm}
          disabled={busy || confirmDisabled}
        >
          {busy ? "Working..." : confirmLabel}
        </button>
      </div>
    </dialog>
  );
}

export default ConfirmDialog;
