import type { ReactNode } from "react";
import { Check } from "lucide-react";

// Minimal markdown for our deterministic Meeting Pack output: hierarchy,
// tables, bullets, checkboxes, bold text, and grounded source jumps.
export function MdBlock({
  md,
  onSource,
}: {
  md: string;
  onSource?: (source: string) => void;
}) {
  const sourcePattern = /^\[(?:(\d{2,}:\d{2})(?:-\d{2,}:\d{2})?|(notes))\]$/i;
  const inline = (s: string) => {
    const parts = s.split(
      /(\*\*[^*]+\*\*|\[(?:\d{2,}:\d{2}(?:-\d{2,}:\d{2})?|notes)\])/gi,
    );
    return parts.map((part, i) => {
      const bold = part.startsWith("**") && part.endsWith("**");
      const content = bold ? part.slice(2, -2) : part;
      const sourceMatch = content.match(sourcePattern);
      const source = sourceMatch?.[1] ?? sourceMatch?.[2];
      if (source) {
        const label =
          source.toLowerCase() === "notes" ? "My notes" : content.slice(1, -1);
        if (!onSource) {
          return (
            <span key={i} className="summary-source static">
              {label}
            </span>
          );
        }
        return (
          <button
            key={i}
            type="button"
            className="summary-source"
            onClick={() => onSource(source)}
            aria-label={
              source.toLowerCase() === "notes"
                ? "Open My Notes"
                : `Open transcript at ${source}`
            }
          >
            {label}
          </button>
        );
      }
      return bold ? <strong key={i}>{content}</strong> : part;
    });
  };
  const lines = md.split("\n");
  const out: ReactNode[] = [];
  let list: ReactNode[] = [];
  let key = 0;
  const flush = () => {
    if (list.length) {
      out.push(<ul key={key++}>{list}</ul>);
      list = [];
    }
  };
  const tableCells = (line: string) =>
    line
      .trim()
      .replace(/^\||\|$/g, "")
      .split("|")
      .map((cell) => cell.trim());
  const tableDivider = (line: string) => {
    const cells = tableCells(line);
    return cells.length > 0 && cells.every((cell) => /^:?-{3,}:?$/.test(cell));
  };
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const line = lines[lineIndex];
    const t = line.trim();
    if (
      t.startsWith("|") &&
      lines[lineIndex + 1] &&
      tableDivider(lines[lineIndex + 1])
    ) {
      flush();
      const headers = tableCells(t);
      const rows: string[][] = [];
      let rowIndex = lineIndex + 2;
      while (
        rowIndex < lines.length &&
        lines[rowIndex].trim().startsWith("|")
      ) {
        rows.push(tableCells(lines[rowIndex]));
        rowIndex += 1;
      }
      out.push(
        <div className="meeting-pack-table-wrap" key={key++}>
          <table className="meeting-pack-table">
            <thead>
              <tr>
                {headers.map((header, index) => (
                  <th key={index}>{inline(header)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, rowKey) => (
                <tr key={rowKey}>
                  {headers.map((_, cellKey) => (
                    <td key={cellKey}>{inline(row[cellKey] ?? "")}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      lineIndex = rowIndex - 1;
    } else if (t.startsWith("### ")) {
      flush();
      out.push(<h4 key={key++}>{t.slice(4)}</h4>);
    } else if (t.startsWith("## ")) {
      flush();
      out.push(<h3 key={key++}>{t.slice(3)}</h3>);
    } else if (t.startsWith("- [ ] ") || t.startsWith("- [x] ")) {
      list.push(
        <li key={key++} className="todo">
          <span className="box">
            {t[3] === "x" ? <Check size={11} /> : null}
          </span>
          <span>{inline(t.slice(6))}</span>
        </li>,
      );
    } else if (t.startsWith("- ")) {
      list.push(<li key={key++}>{inline(t.slice(2))}</li>);
    } else if (t === "") {
      flush();
    } else {
      flush();
      out.push(<p key={key++}>{inline(t)}</p>);
    }
  }
  flush();
  return <div className="md">{out}</div>;
}
