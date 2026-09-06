# Team design consistency plan

## Direction

Apply the Messages redesign's restrained, native-feeling hierarchy throughout
Team. Use the existing warm tokens and Geist typography. Prioritize content,
consistent navigation, readable controls, and accessible keyboard interaction.

## Implementation

1. Workspace: compact team switcher and account menu; remove the decorative
   tagline. Keep the same primary navigation on every page with clear selection.
2. Meetings: follow `MEETINGS_LAYOUT.md`. Group library navigation, reduce repeated
   introductory text, tighten the question area, and align the meeting list.
3. Messages: retain `MESSAGES_LAYOUT.md` and its responsive list/detail behavior.
4. People: align the page heading and search field with the library, alphabetize
   names, show the filtered count, and provide a clear-filter empty state. Keep
   profile and message actions explicit, including pending/error feedback.
5. Search: lead with the query. Put advanced filters in a disclosure with an
   always-visible active count. Preserve filters, query scope, and result links.
6. Settings: group navigation in wrapping controls instead of a scrolling tab
   strip. Place team renaming within Members rather than above every settings
   page. Retain role-based controls, confirmations, and permission explanations.
7. Shared details: use common page widths, heading sizes, input surfaces, focus
   rings, row density, and responsive wrapping. Keep source/privacy explanations.

## Checks

Format all touched source files. Build and run relevant team regressions. Inspect
synthetic data in both themes and wide/narrow windows. Check navigation, collection
collapse, people filtering, search filters, and settings categories. Do not mutate
real team data. No backend or permission-model changes are part of this work.

## Verification completed

- Frontend build and 17 messaging/search regression tests passed.
- Synthetic preview inspected at 1200px and 600px in light and dark themes.
- Checked collection disclosure, People empty/filter reset states, long names,
  settings category navigation, and filtered Search results/active indicators.
- No horizontal page overflow in the tested narrow layouts. Installed desktop
  review remains for the user; no real team records were changed.
