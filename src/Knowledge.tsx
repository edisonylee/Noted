import { useState } from "react";
import { Users, Network } from "lucide-react";
import { PeopleView } from "./PeopleView";
import { SelfView } from "./Self";

// "Knowledge" groups the two "what noted knows about me" surfaces — People and
// the Self entity graph — behind one destination with a section toggle.
type Section = "people" | "self";

export function KnowledgeView({ theme }: { theme: string }) {
  const [section, setSection] = useState<Section>("people");
  return (
    <div className="knowledge">
      <div className="kn-tabs">
        <button
          className={"kn-tab" + (section === "people" ? " on" : "")}
          onClick={() => setSection("people")}
        >
          <Users size={15} /> People
        </button>
        <button
          className={"kn-tab" + (section === "self" ? " on" : "")}
          onClick={() => setSection("self")}
        >
          <Network size={15} /> Self
        </button>
      </div>
      {section === "people" ? <PeopleView /> : <SelfView theme={theme} />}
    </div>
  );
}
