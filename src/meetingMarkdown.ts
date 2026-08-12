import type { MeetingSegment, MeetingSummary } from "./api";

type SummaryForMarkdown = Pick<MeetingSummary, "template" | "content_md">;

type MeetingMarkdownInput = {
  title: string;
  startedAt?: string | null;
  attendeeNames?: readonly string[];
  summary?: SummaryForMarkdown | null;
  notes?: string;
  segments: readonly MeetingSegment[];
  diarization: boolean;
  captureMode?: "online" | "in_person";
};

function oneLine(value: string): string {
  return value.trim().replace(/\s*\n+\s*/g, " ");
}

function safeTitle(value: string): string {
  return oneLine(value) || "Meeting";
}

function mmss(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

function speakerName(
  segment: MeetingSegment,
  diarization: boolean,
  captureMode: "online" | "in_person" = "online",
): string {
  if (captureMode === "in_person") return oneLine(segment.speaker ?? "") || "Speaker";
  if (segment.channel === "me") return "Me";
  return diarization ? oneLine(segment.speaker ?? "") || "Them" : "Them";
}

function transcriptLines(
  segments: readonly MeetingSegment[],
  diarization: boolean,
  captureMode: "online" | "in_person" = "online",
): string[] {
  return [...segments]
    .sort((a, b) => a.t0_ms - b.t0_ms || a.id - b.id)
    .map((segment) =>
      `- [${mmss(segment.t0_ms)}] **${speakerName(segment, diarization, captureMode)}**: ${oneLine(segment.text)}`
    );
}

function demoteSummaryHeadings(markdown: string): string {
  return markdown
    .trim()
    .split("\n")
    .map((line) => (line.startsWith("## ") ? `### ${line.slice(3)}` : line))
    .join("\n");
}

/** A clipboard-friendly Markdown document containing only the full transcript. */
export function transcriptMarkdown(
  title: string,
  segments: readonly MeetingSegment[],
  diarization: boolean,
  captureMode: "online" | "in_person" = "online",
): string {
  const lines = transcriptLines(segments, diarization, captureMode);
  return `# ${safeTitle(title)}\n\n## Transcript\n\n${lines.join("\n")}\n`;
}

/**
 * A concise AI handoff: one selected summary, the user's notes, and the full
 * transcript. It deliberately avoids including every overlapping summary tab.
 */
export function meetingMarkdown({
  title,
  startedAt,
  attendeeNames = [],
  summary,
  notes = "",
  segments,
  diarization,
  captureMode = "online",
}: MeetingMarkdownInput): string {
  const parts = [`# ${safeTitle(title)}`];
  const meta = [startedAt?.slice(0, 10) ?? "", ...attendeeNames.map(oneLine)].filter(Boolean);
  if (meta.length > 0) parts.push(`*${meta.join(" · ")}*`);

  if (summary?.content_md.trim()) {
    parts.push(`## ${safeTitle(summary.template)}`, demoteSummaryHeadings(summary.content_md));
  }
  if (notes.trim()) parts.push("## My Notes", notes.trim());

  const lines = transcriptLines(segments, diarization, captureMode);
  if (lines.length > 0) parts.push("## Transcript", lines.join("\n"));

  return `${parts.join("\n\n")}\n`;
}
