# Data Table Guide

Read this guide whenever a task creates or substantially changes a table,
data grid, admin list, audit log, or other tabular view in a Next.js app.

The required result is a shadcn/ui Data Table with a bounded viewport, a sticky
header, a single shadcn `ScrollArea`, seamless incremental loading, and URL
state that the server can restore after refresh.

## Contents

- [Component foundation](#component-foundation)
- [Required architecture](#required-architecture)
- [Fixed table size](#fixed-table-size)
- [Search and filter toolbar placement](#search-and-filter-toolbar-placement)
- [ScrollArea is the only scroll owner](#scrollarea-is-the-only-scroll-owner)
- [Sticky header](#sticky-header)
- [Backend query contract](#backend-query-contract)
- [URL state and refresh restoration](#url-state-and-refresh-restoration)
- [Seamless incremental loading](#seamless-incremental-loading)
- [Filtering and sorting behavior](#filtering-and-sorting-behavior)
- [Loading, empty, and error states](#loading-empty-and-error-states)
- [Completion checklist](#completion-checklist)

## Component foundation

- Start from the shadcn/ui Data Table recipe: a local
  `components/ui/data-table.tsx` composed from shadcn `Table` primitives and
  TanStack Table.
- Keep column definitions close to the feature, preferably in a dedicated
  `columns.tsx` when they contain substantial rendering logic.
- Keep the reusable Data Table generic. Put feature-specific filters, server
  actions, URL keys, and row actions in the feature layer.
- Reuse the project's existing Data Table before adding another abstraction.
  Extend its props only for behavior shared by multiple tables.

The official shadcn guidance treats each data table as feature-specific. Build
on the shared primitives without turning the generic component into a large
configuration framework.

## Required architecture

Split responsibilities as follows:

1. The Server Component page parses and validates URL search parameters.
2. A server-only query or backend endpoint accepts `page`, `pageSize` or the
   backend's exact `pagesize` spelling, filter values, and server-side sort
   values.
3. The server returns `{ items, total, page, pageSize }` with a deterministic
   ordering and stable row identifiers.
4. The Client Component renders the shadcn Data Table, appends later pages,
   updates the URL, and listens to the `ScrollArea` viewport.
5. Filter controls write to the URL. The URL drives the server query rather
   than duplicating durable table state in local storage.

Use React Query when the surrounding application already uses it or when its
caching and mutation behavior is useful. It does not replace the URL as the
source of truth for pagination, filters, or sorting.

## Fixed table size

The table must have a bounded viewport. Do not let the number of rows determine
the page height.

- Give the page or panel an explicit height such as
  `h-[calc(100dvh-var(--header-height))]`, a fixed design-token height, or a
  fixed parent grid track.
- Carry `h-full`, `flex-1`, and `min-h-0` through every parent between the page
  shell and the Data Table.
- Give the table root `w-full`, `min-h-0`, `flex-1`, and `overflow-hidden`.
- Give the `ScrollArea` `h-full w-full`.
- Use an intentional table width such as `w-full min-w-[800px]`. Add stable
  column widths when the content would otherwise resize the layout while rows
  load.
- Do not rely on `max-height` alone; it permits the table to shrink and makes
  empty/loading states change the page layout.

Typical containment chain:

```tsx
<main className="flex h-[calc(100dvh-3.5rem)] min-h-0 flex-col overflow-hidden">
  <section className="flex min-h-0 flex-1 flex-col gap-4">
    <DataTable className="min-h-0 flex-1 overflow-hidden" />
  </section>
</main>
```

Match the loading skeleton to the same fixed dimensions so loading does not
cause layout shift.

## Search and filter toolbar placement

Place search boxes and filter dropdowns directly above the table viewport,
inside the same fixed table section. The controls must remain visible while
the user scrolls through rows.

- Render the toolbar immediately before the `ScrollArea`; do not put it inside
  the scroll viewport.
- Give the toolbar `shrink-0` and the table wrapper `min-h-0 flex-1` so only the
  rows consume the remaining height.
- Use a responsive layout such as
  `flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center`.
- Put the primary search input first, followed by filter selects, an optional
  reset button, and table actions. Right-align destructive or bulk actions when
  the layout has room.
- Keep the toolbar visually attached to the table. A title or short
  description may appear above it, but do not insert unrelated cards or page
  content between the controls and the table.
- If the shared Data Table exposes `toolbar` or `filters` props, render those
  props immediately above its `ScrollArea`. If filters are feature-specific,
  compose them outside the generic component but keep them as its adjacent
  sibling.

Typical layout:

```tsx
<section className="flex min-h-0 flex-1 flex-col gap-4">
  <div className="flex shrink-0 flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center">
    <Input className="w-full sm:w-64" placeholder="Search..." />
    <Select>{/* filter options */}</Select>
  </div>

  <div className="min-h-0 flex-1">
    <DataTable className="h-full min-h-0" />
  </div>
</section>
```

Commit search and filter values to the URL using the rules below. Debounce text
input; update select filters immediately; reset `page` to `1` for either kind
of filter change.

## ScrollArea is the only scroll owner

Render the table inside shadcn/ui `ScrollArea`; do not use a browser-native
`overflow-auto`, `overflow-x-auto`, or `overflow-y-auto` container for the
table viewport.

The stock shadcn `Table` primitive normally creates a wrapper with
`overflow-x-auto`. Nested inside `ScrollArea`, that wrapper creates a second
scroll owner and can break sticky headers. Resolve it in one of these ways:

- Extend the local `Table` primitive with a `containerClassName` prop and pass
  `overflow-visible` when it is inside `ScrollArea`.
- Render the semantic `<table>` element directly inside `ScrollArea` while
  continuing to use the local shadcn `TableHeader`, `TableBody`, `TableRow`,
  `TableHead`, and `TableCell` primitives.

For new Data Tables, render `ScrollArea` unconditionally. A legacy
`withScrollArea` flag may remain for compatibility, but every newly created
table screen must enable it.

Include shadcn `ScrollBar` for horizontal overflow:

```tsx
<ScrollArea className="h-full w-full" viewportRef={viewportRef}>
  <Table containerClassName="w-full overflow-visible">
    {/* header and body */}
  </Table>
  <ScrollBar orientation="horizontal" />
</ScrollArea>
```

Do not attach infinite-scroll logic to `window`, `document`, the page shell, or
the `ScrollArea` root. Attach it to the actual viewport element exposed through
`viewportRef`.

## Sticky header

Keep the complete header group fixed inside the `ScrollArea` viewport:

```tsx
<TableHeader className="sticky top-0 z-10 border-b border-border bg-background/95 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-background/80">
  {/* header rows */}
</TableHeader>
```

- Ensure header cells have a non-transparent background so body text does not
  show through while scrolling.
- Keep the header inside the same table and scroll viewport as the body so
  columns remain aligned.
- Do not implement a separate duplicated header table.

## Backend query contract

The backend must perform pagination, filtering, and server-relevant sorting.
Use one-indexed `page` values unless an existing API contract requires a
different convention.

Validate and normalize:

- `page`: finite integer, minimum `1`.
- `pageSize`/`pagesize`: finite integer from an allowlist or within a safe
  bounded range.
- text filters: trimmed and length-limited.
- enum filters and sort keys: allowlisted.
- empty or default filters: normalized consistently.

Return at least:

```ts
type TablePage<T> = {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
};
```

Use deterministic ordering with a unique tiebreaker, for example
`createdAt desc, id desc`, so adjacent page requests cannot randomly duplicate
or skip rows.

Map URL naming to the backend at the boundary. Prefer consistent URL keys such
as `page` and `pageSize`; if the backend requires `pagesize` or `page_size`,
translate it in the server query client instead of leaking multiple spellings
through the UI.

## URL state and refresh restoration

Persist every value that changes the visible dataset:

- loaded `page`;
- base `pageSize`;
- search text and active filters;
- server-side sorting, when present.

Use `URLSearchParams` to preserve unrelated parameters. Delete default or empty
values to keep URLs readable.

When a filter, sort, or `pageSize` changes:

1. Reset `page` to `1`.
2. Replace the accumulated rows with the new first page.
3. Reset the `ScrollArea` viewport to the top.
4. Update the URL with `router.replace(..., { scroll: false })` so the Server
   Component receives the new query.

Debounce free-text filters before navigation. Selects and explicit submit
buttons can update immediately.

When seamless scrolling successfully appends a page, update the saved `page`
without remounting the table or moving the scroll position. A native
`window.history.replaceState` update is appropriate for this progress marker;
perform it only after the request succeeds.

On refresh, `page=N` means the user had loaded the prefix through page `N`.
Restore that entire prefix, not only page `N`:

- If the backend supports a prefix query, request page `1` with an effective
  limit of `N * pageSize`.
- Otherwise fetch pages `1..N`, combine them in order, and deduplicate by the
  stable row ID.

This restoration rule preserves the same visible rows after a refresh.

## Seamless incremental loading

Treat “seamless scrolling” as infinite incremental loading inside the fixed
`ScrollArea`, not an animated marquee.

- Trigger the next request when the viewport is within roughly `100-200px` of
  the bottom, or use an `IntersectionObserver` sentinel whose root is the
  `ScrollArea` viewport.
- Guard requests with `loading` and `hasMore` so one threshold crossing cannot
  start duplicate fetches.
- Request `nextPage = page + 1` with the current `pageSize`, filters, and sort.
- Append rows after success and deduplicate them by a stable ID.
- Keep the current data visible while loading. Do not replace it with a full
  table spinner.
- Set `hasMore` from `loadedRows.length < total`; also stop when the returned
  page is empty or shorter than `pageSize`.
- Preserve row order and scroll position. Do not call `router.refresh()` after
  each appended page.
- Show a compact loading-more indicator and an accessible end-of-results
  message outside the scrolling body or in a non-disruptive footer row.

Use `useCallback` for `loadMore` and any component-local URL update function,
consistent with this skill's component architecture rules. Clean up scroll
listeners or observers in the effect cleanup.

## Filtering and sorting behavior

- Send filtering and sorting parameters to the backend for datasets that are
  paginated remotely. Do not filter only the currently loaded client rows and
  present that as a complete result.
- Keep temporary input text local only while debouncing. The committed filter
  value belongs in the URL.
- Reset pagination and accumulated rows whenever any dataset-defining
  parameter changes.
- Keep TanStack Table in manual/server mode for remotely paginated sorting and
  filtering. Use it for column definitions, rendering, visibility, selection,
  and interaction state rather than client-side pagination of an incomplete
  dataset.

## Loading, empty, and error states

- Use a shadcn `Skeleton` matching the fixed table shell for initial server
  loading.
- Keep an existing table visible during load-more requests.
- Show the empty state inside a full-width table row with the correct
  `colSpan`.
- Surface load-more errors without discarding already loaded rows, and provide
  a retry action.
- Disable repeated automatic retries until the user scrolls again or chooses
  retry.

## Completion checklist

Before finishing a table task, verify all of the following:

- The feature uses the local shadcn Data Table pattern.
- The table has an explicit bounded height and width behavior.
- Search inputs and filter selects sit directly above the table in a
  non-scrolling `shrink-0` toolbar.
- The table is wrapped in shadcn `ScrollArea`.
- No nested native overflow container owns table scrolling.
- The header remains fixed while body rows scroll.
- The infinite-scroll listener targets the `ScrollArea` viewport.
- Later pages append without scroll jumps or duplicate requests.
- Backend pagination, filters, and sorting share one validated contract.
- URL state contains pagination and every active dataset filter.
- Filter changes reset the page and scroll position.
- Refreshing at `page=N` reconstructs rows from pages `1..N`.
- Initial loading uses a dimension-matched shadcn skeleton.
