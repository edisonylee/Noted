// Renders arbitrary extracted JSON as a readable nested view.
// Arrays of objects become compact tables; everything else becomes key/value rows.

// Column order for tables: identifier first, then time fields in a natural order,
// then everything else as encountered. Without this, the model's key order leaks
// through (e.g. "end" landing before "start").
const COL_PRIORITY = ["name", "task", "label", "title", "item", "start", "end", "duration_min"];

function orderColumns(cols: string[]): string[] {
  const present = new Set(cols);
  const front = COL_PRIORITY.filter((k) => present.has(k));
  const frontSet = new Set(front);
  return [...front, ...cols.filter((c) => !frontSet.has(c))];
}

// Known units appended in the display only (data stays a bare number).
const UNIT: Record<string, string> = { weight: "lb" };
function unitSuffix(key: string, v: unknown): string {
  return UNIT[key] && typeof v === "number" ? ` ${UNIT[key]}` : "";
}

export function DataView({ value }: { value: unknown }) {
  return <div className="dataview">{render(value)}</div>;
}

function render(value: unknown): React.ReactNode {
  if (value === null || value === undefined) return <span className="muted">—</span>;

  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="muted">empty</span>;
    // array of flat objects -> table
    if (value.every((v) => isPlainObject(v))) {
      const cols = orderColumns(
        Array.from(
          value.reduce<Set<string>>((acc, row) => {
            Object.keys(row as object).forEach((k) => acc.add(k));
            return acc;
          }, new Set())
        )
      );
      return (
        <div className="dv-table-wrap">
          <table className="dv-table">
            <thead>
              <tr>{cols.map((c) => <th key={c}>{c}</th>)}</tr>
            </thead>
            <tbody>
              {value.map((row, i) => (
                <tr key={i}>
                  {cols.map((c) => {
                    const cell = (row as Record<string, unknown>)[c];
                    return (
                      <td key={c}>
                        {scalarOr(cell)}
                        {unitSuffix(c, cell)}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    return (
      <ul className="dv-list">
        {value.map((v, i) => <li key={i}>{render(v)}</li>)}
      </ul>
    );
  }

  if (isPlainObject(value)) {
    return (
      <div className="dv-obj">
        {Object.entries(value as Record<string, unknown>).map(([k, v]) => (
          <div className="dv-row" key={k}>
            <span className="dv-key">{k}</span>
            <span className="dv-val">
              {isLeaf(v) ? scalarOr(v) : render(v)}
              {unitSuffix(k, v)}
            </span>
          </div>
        ))}
      </div>
    );
  }

  return <span className="dv-scalar">{String(value)}</span>;
}

function scalarOr(v: unknown): React.ReactNode {
  if (v === null || v === undefined || v === "") return <span className="muted">—</span>;
  if (isLeaf(v)) return String(v);
  return render(v);
}

function isLeaf(v: unknown): boolean {
  return v === null || typeof v !== "object";
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
