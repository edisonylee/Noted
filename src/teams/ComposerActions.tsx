import { useRef, useState } from "react";
import { BookOpen, FileText, Paperclip, Plus } from "lucide-react";
import { useOutsideDismiss } from "../ui/useDismissal";
import { composerActions, type ComposerAction } from "./composerCommands";
import "./composer-actions.css";

export function ActionIcon({ action }: { action: ComposerAction }) {
  const Icon =
    action === "meeting"
      ? BookOpen
      : action === "document"
        ? FileText
        : Paperclip;
  return <Icon size={16} aria-hidden="true" />;
}

export function ComposerActions({
  available,
  disabled,
  onChoose,
}: {
  available: ComposerAction[];
  disabled: boolean;
  onChoose: (action: ComposerAction) => void;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  useOutsideDismiss(open, [trigger, popup], () => setOpen(false));
  if (!available.length) return null;
  const actions = composerActions.filter((action) =>
    available.includes(action.id),
  );
  const show = () => {
    setOpen(true);
    requestAnimationFrame(() =>
      popup.current?.querySelector<HTMLButtonElement>("button")?.focus(),
    );
  };
  return (
    <div className="composer-actions">
      <button
        ref={trigger}
        type="button"
        className="icon-btn messages-compose-attach"
        aria-label="Add to message"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            show();
          }
        }}
      >
        <Plus size={17} aria-hidden="true" />
      </button>
      {open && !disabled && (
        <div
          ref={popup}
          role="menu"
          aria-label="Add to message"
          className="composer-action-menu"
          onKeyDown={(event) => {
            const buttons = [
              ...popup.current!.querySelectorAll<HTMLButtonElement>("button"),
            ];
            const current = buttons.indexOf(
              document.activeElement as HTMLButtonElement,
            );
            if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
              event.preventDefault();
              const index =
                event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? buttons.length - 1
                    : (current +
                        (event.key === "ArrowDown" ? 1 : buttons.length - 1)) %
                      buttons.length;
              buttons[index]?.focus();
            }
            if (event.key === "Tab") setOpen(false);
          }}
        >
          {actions.map((action) => (
            <button
              type="button"
              role="menuitem"
              key={action.id}
              onClick={() => {
                setOpen(false);
                trigger.current?.focus();
                onChoose(action.id);
              }}
            >
              <ActionIcon action={action.id} />
              <span>
                {action.label}
                <small>{action.description}</small>
              </span>
              <kbd>/{action.id}</kbd>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
