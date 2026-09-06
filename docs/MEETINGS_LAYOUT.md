# Meetings layout

Bring the shared meeting library into the same visual family as Messages:
compact navigation, warm surfaces, clear hierarchy, and consistent focus states.

## Plan

- Label the sidebar Library. Keep All meetings first, collections in a collapsible
  group with one create action, and Saved answers / Trash together below.
- Retain collection access labels and folder hierarchy. Truncate long navigation
  names with full-name tooltips. Indicate the current destination accessibly.
- Remove the redundant page eyebrow and shorten introductory copy. Keep sharing
  prominent, with collection permissions visible beneath the title.
- Reduce the question area's spacing while retaining source scope, question
  history, and the privacy explanation. Keep the meeting list easy to reach.
- Give the meeting filter a clear input surface. Align meeting rows, limit
  excerpts, and make keyboard focus as visible as pointer hover.
- Use responsive navigation and wrapping controls on narrow windows. Scope styles
  to Meetings so Messages and other team views retain their layouts.

## Validation

Run formatting and frontend build checks. Inspect wide/narrow layouts and both
themes with synthetic data. Preserve sharing, collection permissions, selection,
questions, saved answers, and trash behavior; no backend changes are needed.
