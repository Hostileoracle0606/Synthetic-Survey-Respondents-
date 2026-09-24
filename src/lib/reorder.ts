/**
 * Step 3 reordering. `moveBefore` gives the order after dragging `id` onto the slot before
 * `before` (null = the end); the result goes to `reorder_questions` as is.
 */
export function moveBefore(ids: number[], id: number, before: number | null): number[] {
  if (id === before || !ids.includes(id)) return ids;
  const rest = ids.filter((x) => x !== id);
  const at = before == null ? -1 : rest.indexOf(before);
  if (at < 0) return [...rest, id];
  return [...rest.slice(0, at), id, ...rest.slice(at)];
}

/** Whether two orders are the same, so a drop that changes nothing makes no call. */
export function sameOrder(a: number[], b: number[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}
