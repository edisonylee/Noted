export type ReleaseProfileName = "development" | "alpha";

const requestedProfile = import.meta.env.VITE_NOTED_PROFILE;

export const releaseProfileName: ReleaseProfileName =
  requestedProfile === "alpha" ? "alpha" : "development";

const isAlpha = releaseProfileName === "alpha";

/**
 * Product capabilities for the current build.
 *
 * Development keeps the full workshop available. The Mac alpha intentionally
 * ships one focused product: notes + meetings + themes + Calendar with Local or
 * BYOK inference. Deferred features stay in the private source tree without
 * appearing in the release UI.
 */
export const releaseProfile = {
  name: releaseProfileName,
  meetingRecording: true,
  systemAudio: true,
  themes: true,
  providerMatrix: true,
  googleCalendar: true,
  balancedInference: !isAlpha,
  notedHosted: !isAlpha,
  // Paused until phone access is a real companion app instead of a LAN-served
  // remote view of the Mac interface.
  phoneLan: false,
  diarization: !isAlpha,
  videoCapture: true,
  billing: !isAlpha,
} as const;

export const isAlphaRelease = isAlpha;
