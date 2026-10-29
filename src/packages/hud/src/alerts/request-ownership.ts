/** Sampling a hidden/non-owning display must not consume a waiting request. */
export function newOwnedRequests<T extends { id: string }>(
  requests: T[],
  seen: Set<string>,
  ownsDisplay: boolean,
): T[] {
  const active = new Set(requests.map((request) => request.id));
  for (const id of seen) if (!active.has(id)) seen.delete(id);
  if (!ownsDisplay) return [];
  const fresh = requests.filter((request) => !seen.has(request.id));
  for (const request of fresh) seen.add(request.id);
  return fresh;
}
