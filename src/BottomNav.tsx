import { CalendarDays, PenLine, Clock, MessageCircle } from "lucide-react";

export type MobileTab = "today" | "capture" | "timeline" | "ask";

const TABS = [
  { id: "today", label: "Today", Icon: CalendarDays },
  { id: "capture", label: "Capture", Icon: PenLine },
  { id: "timeline", label: "Timeline", Icon: Clock },
  { id: "ask", label: "Ask", Icon: MessageCircle },
] as const;

export function BottomNav({
  active,
  onChange,
}: {
  active: MobileTab;
  onChange: (t: MobileTab) => void;
}) {
  return (
    <nav className="bottom-nav">
      {TABS.map(({ id, label, Icon }) => (
        <button
          key={id}
          className={"bn-item" + (active === id ? " on" : "")}
          onClick={() => onChange(id)}
          aria-label={label}
        >
          <Icon size={21} strokeWidth={2} />
          <span>{label}</span>
        </button>
      ))}
    </nav>
  );
}
