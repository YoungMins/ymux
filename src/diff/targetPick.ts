export type Target = { kind: "one"; id: string } | { kind: "many"; ids: string[] } | { kind: "none" };

/// Who receives the review: the only candidate, the one used last time if it
/// is still a candidate, otherwise the user has to choose.
export function chooseTarget(ids: string[], lastChoice?: string | null): Target {
  if (ids.length === 0) return { kind: "none" };
  if (ids.length === 1) return { kind: "one", id: ids[0] };
  if (lastChoice != null && ids.includes(lastChoice)) return { kind: "one", id: lastChoice };
  return { kind: "many", ids };
}
