// Shared "last used" interpretation — UsageBadge renders it per-row and the
// Optimize wizard's review callout aggregates it; both must agree on what
// counts as stale.
//
// The data comes from `dumpsys usagestats`, which prints only Android's
// in-memory buckets — and Android rebuilds those at boot. Every bucket, the
// "yearly" one included, starts at the last reboot. So the whole dataset only
// describes the window since boot, and "no record" means "not opened since the
// device came up", NOT "never used". On a box rebooted an hour ago that is true
// of almost every app, which is why staleness has to be judged against the
// observed window rather than asserted outright.

import type { AppUsage } from "./types";

const DAY_MS = 86_400_000;

/// Days without a launch before an app reads as a removal candidate.
export const STALE_DAYS = 30;

/// Days since last foreground use, or `null` when there's no usable record.
export function daysSinceUsed(u: AppUsage | undefined): number | null {
  if (!u || !u.last_used || u.launch_count === 0) return null;
  // "YYYY-MM-DD HH:MM:SS" → parse as local time (device-local, close enough).
  const then = new Date(u.last_used.replace(" ", "T"));
  if (Number.isNaN(then.getTime())) return null;
  return Math.floor((Date.now() - then.getTime()) / DAY_MS);
}

/// Device uptime in days — how much history `dumpsys usagestats` can describe.
/// `null` when the device didn't report uptime.
export function windowDays(windowSecs: number | null | undefined): number | null {
  if (windowSecs == null || !Number.isFinite(windowSecs)) return null;
  return windowSecs / 86_400;
}

/// Can "no record" be read as "unused" yet? Only once the device has been up
/// long enough for the absence to mean something. Below that the honest answer
/// is "we don't know", not "never used".
export function windowIsConclusive(windowSecs: number | null | undefined): boolean {
  const days = windowDays(windowSecs);
  return days !== null && days >= STALE_DAYS;
}

export function usageLabel(u: AppUsage | undefined, windowSecs?: number | null): string {
  const days = daysSinceUsed(u);
  if (days === null) {
    return windowIsConclusive(windowSecs) ? "no recent use" : "not used since reboot";
  }
  if (days <= 0) return "used today";
  if (days === 1) return "used yesterday";
  if (days < 30) return `last used ${days}d ago`;
  if (days < 365) return `last used ${Math.floor(days / 30)}mo ago`;
  return `last used ${Math.floor(days / 365)}y ago`;
}

/// Terse variant for the App List's own "Last used" column, where the column
/// header already supplies the "last used" part of `usageLabel`'s phrasing.
export function usageShortLabel(u: AppUsage | undefined, windowSecs?: number | null): string {
  const days = daysSinceUsed(u);
  if (days === null) {
    return windowIsConclusive(windowSecs) ? "no recent use" : "not since reboot";
  }
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

/// Stale = a removal candidate: untouched for 30+ days, or no record at all
/// over a window long enough for that absence to be meaningful. A short window
/// is never stale — a reboot must not turn every app into a removal candidate.
export function isStaleUsage(u: AppUsage | undefined, windowSecs?: number | null): boolean {
  const days = daysSinceUsed(u);
  if (days === null) return windowIsConclusive(windowSecs);
  return days >= STALE_DAYS;
}

/// Plain-English description of how much history exists, for tooltips.
export function windowNote(windowSecs: number | null | undefined): string {
  const days = windowDays(windowSecs);
  if (days === null) {
    return "Android rebuilds usage history at every reboot, so this only covers the time since the device last booted.";
  }
  const human =
    days < 1
      ? `${Math.max(1, Math.round(days * 24))} hour(s)`
      : `${Math.floor(days)} day(s)`;
  return `Android rebuilds usage history at every reboot — this device has been up ${human}, so that's all the history there is.`;
}
