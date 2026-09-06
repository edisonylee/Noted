# Floating dock navigation

The desktop app uses a 56 px floating capsule with 44 px button targets,
20 px destination icons, and an active accent marker. Seven primary destinations follow current master: Home, Schedule, Calendar,
Documents, Library, Team, and Knowledge. Their labels, icons, and order live in
`src/navigation.ts`, shared by the dock, overlay, and preview.
The Aperture icon opens a modal navigation panel; the microphone opens
recording options or the active recording.

The panel contains existing navigation and utility controls, plus search over
destinations and the documents and captures already loaded by the app. Results exclude trashed
items and show up to eight, most recently updated first. Each result identifies
its kind. Selecting a document opens DocumentsView; a capture opens LibraryView.
Meeting and transcript searches remain in the existing Library workspace. The note library remains a separate contextual panel.

- Click Aperture or press Command-K outside editable text to open navigation.
- Command-Shift-K also works while editing, preserving the editor's link shortcut.
- Enter opens the first matching destination or note; arrow keys move through results.
- Escape, the close button, or the backdrop dismisses the panel.
- Native dialog behavior keeps keyboard focus inside the panel and focus returns on close.
- Hover and keyboard focus reveal dock labels. Motion and translucency have reduced-preference fallbacks.

Implementation: `src/FloatingDock.tsx` and `src/FloatingDock.css`, with small
integration changes in the current App and LibraryWorkspace. Team notifications, TeamWorkspace,
Document/Library separation, and the newer schedule and graph layouts stay in place. Existing theme tokens color the
surface; the four colors remain confined to the Aperture artwork. The overlay
starts closed and does not change saved sidebar preferences.

## Preview and validation

Run `bunx vite --host 127.0.0.1 --port 4179 --strictPort`, then open
`http://127.0.0.1:4179/dock-preview.html`. This preview uses the actual dock
component with explicit sample notes; recording there is a visual toggle.

Browser verification covered the seven-item order and matching Lucide icons,
search-result kind labels, capture-to-Library navigation, document-to-Documents
navigation, and Team selection. The integrated app opened its existing Team,
Documents, and Library screens. The seven-item overlay fits at the standard
1280 × 720 preview size, and the dock remains scrollable in short windows.

The browser shell was disconnected from the native backend. No live recording,
team connection, or real-data write was performed. The combined standard macOS app was subsequently rebuilt and installed with
the floating dock, Aperture icon, neon themes, and companion. Native verification
confirmed the pet chat launcher, customization, desktop detachment, and return.
Type checking and production builds passed with the existing large-chunk warning.
