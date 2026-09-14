export type MergeInterval = { a: string; b: string; start: number; end: number };

export const mergeKey = (a: string, b: string) => JSON.stringify([a, b]);

/** Longest selected connected chain, independent of the display/selection order. */
export function longestMergeChain(selected: readonly MergeInterval[]): number {
  const next = new Map(selected.map((s) => [s.a, s]));
  const incoming = new Set(selected.map((s) => s.b));
  const visited = new Set<string>();
  let longest = 0;
  for (const first of selected) {
    if (incoming.has(first.a)) continue;
    let row: MergeInterval | undefined = first;
    let end = first.end;
    while (row) {
      if (visited.has(row.a) || !Number.isFinite(row.start) || !Number.isFinite(row.end) || row.end <= row.start) return Infinity;
      visited.add(row.a);
      end = Math.max(end, row.end);
      row = next.get(row.b);
    }
    longest = Math.max(longest, end - first.start);
  }
  return visited.size === selected.length ? longest : Infinity;
}

/** Detach handlers too: a late loadedmetadata must not restart a retired clip. */
export function releaseBoundaryAudio(audio: HTMLAudioElement | null) {
  if (!audio) return;
  audio.onloadedmetadata = null;
  audio.ontimeupdate = null;
  audio.onended = null;
  audio.onerror = null;
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
}
