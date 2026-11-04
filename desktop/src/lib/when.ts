// How long since `iso`, as `teer pr` says it: `now`, `5m`, `3h`, `2d`, `6w`.
export function ago(iso: string | undefined, now: Date = new Date()): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const mins = Math.max(0, Math.floor((now.getTime() - then) / 60000));
  if (mins === 0) return "now";
  if (mins < 60) return `${mins}m`;
  if (mins < 60 * 24) return `${Math.floor(mins / 60)}h`;
  if (mins < 60 * 24 * 14) return `${Math.floor(mins / (60 * 24))}d`;
  return `${Math.floor(mins / (60 * 24 * 7))}w`;
}
