import { useEffect, useRef, type ReactNode } from "react";

export function TeamDialog({
  title,
  children,
  onClose,
  busy = false,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  busy?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    d?.showModal();
    d?.querySelector<HTMLElement>(
      "input:not([type=checkbox]), textarea, select",
    )?.focus();
    return () => d?.close();
  }, []);
  return (
    <dialog
      ref={ref}
      className="team-dialog"
      aria-label={title}
      onCancel={(e) => {
        e.preventDefault();
        if (!busy) onClose();
      }}
    >
      <header>
        <h2>{title}</h2>
        <button
          className="team-text-button"
          disabled={busy}
          onClick={onClose}
          aria-label="Close dialog"
        >
          Close
        </button>
      </header>
      {children}
    </dialog>
  );
}
