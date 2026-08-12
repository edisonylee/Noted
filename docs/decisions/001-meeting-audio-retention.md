# Decision 001: Meeting audio retention and compression

Status: accepted

Date: 2026-08-06

Owners: product and meeting architecture

Implementation status: planned; the existing retention boolean and raw WAV path
are partial foundations, not completion of this decision.

## Context

Noted needs live microphone and system audio to create a transcript. That capture
requirement is separate from whether Noted retains the audio after processing.
Calling both behaviors “recording” makes privacy controls confusing.

Retained audio enables timestamp playback, transcript verification, speaker
repair, and debugging, but uncompressed dual-channel WAV files consume substantial
disk space. Production users may want the transcript without a permanent audio
recording. During current development, retained source audio remains valuable for
diagnosing speaker attribution.

## Decision

### User model

Noted exposes two audio policies for each meeting:

- `transcript_only` — audio is captured for transcription and then discarded.
- `keep_audio` — audio is retained locally in compressed form for playback and
  later repair.

User-facing copy should describe this as **Audio after the meeting** or **Keep
audio after meeting**, not as a choice to record the meeting.

A global default initializes each meeting, and a visible per-meeting control can
override it without changing the global preference. The resolved policy is stored
as an immutable snapshot on the meeting so a later settings change cannot alter
historical behavior.

The policy cannot be upgraded after capture begins because earlier audio cannot be
recovered. A retained recording can be deleted after the meeting without deleting
the transcript, summary, citations, or speaker corrections.

### Defaults

- Fresh production installs default to `transcript_only`.
- Existing installations retain their saved preference during migration.
- Interactive development and speaker-debugging builds default to `keep_audio`.
- Automated tests set the policy explicitly and do not depend on build defaults.
- A product demo may explicitly use a meeting with retained audio to demonstrate
  timestamp playback. It must not silently change the production privacy default
  to create an upsell.

### Local media

When audio is retained:

1. Preserve microphone and system audio as separate tracks.
2. Capture temporary 16 kHz mono WAV sources using the existing pipeline.
3. After the meeting, encode each track as AAC-LC in an M4A container at roughly
   48 kbps per track.
4. Verify that the outputs exist, decode, and have a plausible duration before
   switching stored paths or deleting a source.
5. If compression fails, keep the WAV sources, mark the media retryable, and never
   destroy the only retained copy.
6. During the current attribution-debugging period, debug builds may preserve raw
   WAV sources temporarily even after verified compression. Production builds do
   not retain those sources after successful conversion.

Two 48 kbps tracks require roughly 40–45 MB per meeting-hour, compared with about
230 MB per hour for two uncompressed 16 kHz, 16-bit mono tracks.

Speaker repair must resolve the stored codec. If its current analysis path
requires WAV, it may decode the compressed system-audio track to a temporary local
WAV and remove that file after repair.

### Cloud behavior

This decision does not upload recordings. Local retention and cloud backup are
separate choices. A future Noted Cloud plan may sync retained compressed audio
only after explicit cloud enrollment. Google Drive may later mirror/export the
compressed files. Neither path changes the meeting's original audio policy.

### Permission behavior

Transcript-only still requires microphone and system-audio access while the
meeting is being transcribed. The per-meeting retention control cannot eliminate
the operating-system capture permission. Repeated permission or capture prompts
are a separate reliability issue and must not be “fixed” by obscuring the capture
requirement.

Video retention remains an independent choice.

## Implementation alignment

The current command boundary already accepts an optional `retainAudio` override in
`src/api.ts`, and the backend falls back to the saved global setting in
`src-tauri/src/lib.rs`. Implementation should evolve that boolean boundary toward
the semantic policy while temporarily accepting the legacy field.

The start surfaces that must resolve and display the policy are:

- the automatic meeting prompt;
- the calendar/pre-meeting page; and
- the sidebar quick-record action.

The quick action should preserve one-click capture by using the saved default and
offer the alternate policy through a secondary control.

Meeting persistence must distinguish policy, media lifecycle state, codec, byte
size, paths, and deletion time. Schema changes remain additive per repository
convention. Any new backend command must be added to the Tauri command, handler
registry, phone dispatch, and frontend API bridge.

## Consequences

### Positive

- The privacy-preserving production behavior is explicit.
- Timestamp evidence remains available when a user values it.
- Compression makes local and future cloud retention economically practical.
- Separate tracks preserve the strongest inputs for attribution and repair.
- The stored snapshot makes support, deletion, and future sync behavior auditable.

### Costs and risks

- A user cannot enable retention mid-meeting and recover earlier audio.
- Compression, verification, retry, crash recovery, and codec-aware repair add a
  media lifecycle.
- Transcript-only meetings cannot offer later audio evidence.
- OS capture permissions remain necessary even when no audio is retained.

## Deferred decisions

- Exact raw-source debug cleanup interval.
- Whether later platforms use AAC/M4A or a second codec behind the same media
  contract.
- Cloud encryption, quotas, regions, and retention policies.
- Whether phone playback streams local media over authenticated byte ranges.
