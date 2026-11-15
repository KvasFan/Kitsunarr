# Quality-profile ordering and grouping — 15 November 2026

The profile editor now supports ordering and group composition in the browser, following
Tanukarr's `EditQualityProfileModalContent.tsx`, `useQualityProfileDnd.ts`,
`QualityProfileItem.tsx` and `QualityProfileItemGroup.tsx` at reference revision
`24382f1f7f23ca13d991e5416d257f631f606529`. The reference files were read-only and clean.
This completes the editor gap identified in the earlier
[quality-profile verification](quality-profile-verification-2026-11-15.md).

## Editing workflow

- Drag the handles to reorder individual qualities or whole groups. The list shows the best
  preference first; storage remains worst-to-best, as in Tanukarr.
- Open **Edit groups**, select **Group** on an individual quality, then drag qualities into
  the new group or choose it in their **Group** selectors. Group names can be edited inline.
- Move members between groups, change their display order or choose **No group** to extract
  one quality. **Ungroup** dissolves a whole group back into individual qualities.
- Arrow buttons and native selectors offer equivalent operations without requiring drag/drop.
  A group's allowed switch applies to its members; grouped qualities rank equally.

Creating a group initially leaves a one-member draft, as in Tanukarr. Saving is disabled
until it has at least two members and a name. Dragging a group over another group reorders
it; groups cannot be nested. Removing members from an existing group automatically dissolves
an empty/single-member remainder. All quality identities, size overrides, codec factors and
custom-format scores are preserved.

Cutoff behavior is explicit: grouping a cutoff quality selects its new group; ungrouping a
cutoff group selects its highest member. If a cutoff group shrinks to one member, that quality
becomes the cutoff. Extracting the sole member of a new draft retains that member as cutoff.
If the cutoff becomes disallowed, the highest allowed top-level item is selected; allowing
no qualities blocks saving. These cutoff/normalization safeguards are Kitsunarr behavior,
rather than a claim of identical intermediate draft state to Tanukarr.

The editing operations live in `kitsunarr-domain::quality_profile`, shared independently of
the UI. They apply atomically: invalid destinations and nested-group attempts leave the
draft unchanged. Persistence uses the existing native and Sonarr-compatible profile APIs.

## Verification

Domain tests exercise group creation, naming, member order, group order, extraction,
dissolution, cutoff changes, metadata preservation and atomic rejection. A matrix of all
move destinations on a representative profile checks that no quality is lost or duplicated.
The draft-only final-member case also has a regression test.

An HTTP test rejects saving a one-member draft, then creates, reorders, saves and reloads a
valid group. It checks v3 member order, grouped size limits, v3 group renaming and native
ungrouping with the expected cutoff. A release-decision regression now uses these same
domain edits to confirm profile order affects ranking and grouped qualities use custom-format
scores to break ties.

Chromium verification uses real WebDriver mouse actions for native drag/drop, alongside
selectors/buttons. It creates and renames a group, adds qualities, reorders members and whole
groups, checks persistence/cutoff after reopening, moves qualities between groups and to the
root, and dissolves the group. During testing, a browser-only cutoff display bug was found:
reordering/replacing options could change the displayed selection despite the correct model
value. Stable option keys and explicit selected states fix that regression.

Final checks: **835 workspace tests passed, 0 failed, 12 ignored**. Workspace and WebAssembly
clippy, formatting/whitespace checks, release web build and server build passed. The full
Chromium scenario passed in **179.33 seconds**, including native mouse drags of an individual
quality into a group, a whole group to a new rank and an extracted individual quality to a
new rank. No synthetic drag/drop events are used. Selector-based extraction, group/member
ordering, save/reopen and cutoff checks also passed.

The updated instance runs at **http://127.0.0.1:8990** under the existing
`kitsunarr-local-8990.service` user service. A SQLite backup was taken first. The native API
responds, and all **7 user profiles** and library counts (**80 series / 14 movies**) match
the pre-update snapshot. Library bookkeeping continued during verification: six episode-file
records were added and one baseline record was removed with the history reason
`MissingFromDisk` (428 records versus 423 initially). Every surviving baseline path is
unchanged. This activity was preserved; the database was not restored over it. No user
profile was edited by the verification; HTTP/browser edits used disposable fixture databases.

## Scope

The comparison is based on Tanukarr's source workflow; Tanukarr's browser was not driven
side-by-side. HTTP/browser mutations use disposable fixture profiles. Existing local user
profiles are preserved during deployment. Drag/drop was exercised in Chromium with a mouse;
the arrow/selector controls remain available for keyboard and touch operation.
