import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Trash2 } from "lucide-react";
import { MdBlock } from "../MeetingMarkdownView";
import { team, orgPath } from "./client";
import type { TeamAnswer } from "./types";

type Saved = TeamAnswer & { id: string; question: string; created_at: string };
type Row = Pick<Saved, "id" | "question" | "created_at"> & {
  available: boolean;
};

export function SavedAnswers({
  org,
  onSource,
}: {
  org: string;
  onSource: (id: string) => void;
}) {
  const [rows, setRows] = useState<Row[]>([]),
    [answer, setAnswer] = useState<Saved | null>(null);
  const [error, setError] = useState(""),
    [loading, setLoading] = useState(true);
  const load = useCallback(async () => {
    setRows(await team.request<Row[]>("GET", orgPath(org, "/answers")));
    setLoading(false);
  }, [org]);
  useEffect(() => {
    let active = true;
    team
      .request<Row[]>("GET", orgPath(org, "/answers"))
      .then((r) => {
        if (active) {
          setRows(r);
          setLoading(false);
        }
      })
      .catch((e) => {
        if (active) {
          setError(String(e));
          setLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [org]);
  useEffect(() => {
    if (!answer) return;
    let active = true;
    const check = () => {
      team
        .request<Saved>("GET", orgPath(org, `/answers/${answer.id}`))
        .catch((e) => {
          if (active) {
            setAnswer(null);
            setError(String(e));
            void load().catch(() => {});
          }
        });
    };
    const timer = window.setInterval(check, 30_000);
    window.addEventListener("focus", check);
    return () => {
      active = false;
      clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [answer, org, load]);
  return (
    <section className="team-saved-answers">
      <header className="team-library-head">
        <div>
          <h1>{answer ? answer.question : "Saved answers"}</h1>
          <p>
            Only your account can open these answers. Source access is checked
            again each time.
          </p>
        </div>
      </header>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
      {answer ? (
        <>
          <button className="team-text-button" onClick={() => setAnswer(null)}>
            <ArrowLeft size={14} /> Saved answers
          </button>
          <div className="team-answer">
            <MdBlock md={answer.answer} />
            <div className="team-sources">
              {answer.sources.map((s) => (
                <button key={s.id} onClick={() => onSource(s.id)}>
                  [{s.citation}] {s.title}
                </button>
              ))}
            </div>
            {answer.limited && (
              <p className="team-muted">
                This answer used a selection of source excerpts.
              </p>
            )}
          </div>
        </>
      ) : (
        <div className="team-note-list">
          {rows.map((row) => (
            <div key={row.id} className="team-note-row">
              <button
                disabled={!row.available}
                onClick={async () => {
                  setError("");
                  try {
                    setAnswer(
                      await team.request<Saved>(
                        "GET",
                        orgPath(org, `/answers/${row.id}`),
                      ),
                    );
                  } catch (e) {
                    setError(String(e));
                    await load();
                  }
                }}
              >
                <strong>{row.question}</strong>
                <span className="team-note-meta">
                  {new Date(row.created_at).toLocaleString()}
                </span>
              </button>
              <button
                className="team-text-button team-delete-answer"
                aria-label={`Delete saved answer: ${row.question}`}
                onClick={async () => {
                  if (
                    !confirm(
                      "Delete this saved answer? Shared meetings are kept.",
                    )
                  )
                    return;
                  try {
                    await team.request(
                      "DELETE",
                      orgPath(org, `/answers/${row.id}`),
                    );
                    await load();
                  } catch (e) {
                    setError(String(e));
                  }
                }}
              >
                <Trash2 size={14} />
              </button>
            </div>
          ))}
          {!rows.length && (
            <p className="team-empty">
              {loading
                ? "Loading saved answers…"
                : "Save an answer after asking your team’s meetings a question."}
            </p>
          )}
          {rows.length === 100 && (
            <p className="team-muted">
              Showing your 100 most recent saved answers.
            </p>
          )}
        </div>
      )}
    </section>
  );
}
