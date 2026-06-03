// Renders arbitrary extracted JSON as a readable nested view.
// Arrays of objects become compact tables; everything else becomes key/value rows.

export function DataView({ value }: { value: unknown }) {
  return <div className="dataview">{render(value)}</div>;
}

function render(value: unknown): React.ReactNode {
  if (value === null || value === undefined) return <span className="muted">—</span>;

  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="muted">empty</span>;
    // array of flat objects -> table
    if (value.every((v) => isPlainObject(v))) {
      const cols = Array.from(
        value.reduce<Set<string>>((acc, row) => {
          Object.keys(row as object).forEach((k) => acc.add(k));
          return acc;
        }, new Set())
      );
      return (
        <table className="dv-table">
          <thead>
            <tr>{cols.map((c) => <th key={c}>{c}</th>)}</tr>
          </thead>
          <tbody>
            {value.map((row, i) => (
              <tr key={i}>
                {cols.map((c) => (
                  <td key={c}>{scalarOr((row as Record<string, unknown>)[c])}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
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
            <span className="dv-val">{isLeaf(v) ? scalarOr(v) : render(v)}</span>
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
