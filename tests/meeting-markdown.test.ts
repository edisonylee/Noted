import { describe, expect, test } from "bun:test";
import type { MeetingSegment } from "../src/api";
import { meetingMarkdown, transcriptMarkdown } from "../src/meetingMarkdown";

function segment(overrides: Partial<MeetingSegment> = {}): MeetingSegment {
  return {
    id: 1,
    channel: "them",
    t0_ms: 2_000,
    t1_ms: 3_000,
    voiced_ms: 700,
    text: "Hello",
    speaker: "Mayan",
    ...overrides,
  };
}

describe("meeting Markdown clipboard output", () => {
  test("copies the complete transcript in chronological, speaker-labeled order", () => {
    const markdown = transcriptMarkdown(
      "Product review",
      [segment(), segment({ id: 2, channel: "me", t0_ms: 1_000, text: "Starting now", speaker: null })],
      true,
    );

    expect(markdown).toBe(
      "# Product review\n\n## Transcript\n\n" +
        "- [00:01] **Me**: Starting now\n" +
        "- [00:02] **Mayan**: Hello\n",
    );
  });

  test("creates a concise AI handoff with one summary, notes, and transcript", () => {
    const markdown = meetingMarkdown({
      title: "Weekly sync",
      startedAt: "2026-08-10T17:00:00Z",
      attendeeNames: ["Jasmine"],
      summary: { template: "Meeting", content_md: "## Decision\nShip Tuesday." },
      notes: "Confirm the launch owner.",
      segments: [segment({ speaker: null })],
      diarization: true,
    });

    expect(markdown).toContain("*2026-08-10 · Jasmine*");
    expect(markdown).toContain("## Meeting\n\n### Decision\nShip Tuesday.");
    expect(markdown).toContain("## My Notes\n\nConfirm the launch owner.");
    expect(markdown).toContain("## Transcript\n\n- [00:02] **Them**: Hello");
  });

  test("uses generic remote labels when diarization is unavailable", () => {
    expect(transcriptMarkdown("Call", [segment({ speaker: "Jasmine" })], false)).toContain(
      "**Them**: Hello",
    );
  });

  test("uses FluidAudio labels for in-person microphone segments", () => {
    const markdown = transcriptMarkdown(
      "Room interview",
      [segment({ channel: "me", speaker: "Speaker 2" })],
      true,
      "in_person",
    );

    expect(markdown).toContain("**Speaker 2**: Hello");
    expect(markdown).not.toContain("**Me**");
  });
});
