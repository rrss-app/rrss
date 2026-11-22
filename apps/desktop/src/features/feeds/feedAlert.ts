import type { FeedView } from "../../bindings";
import { formatFeedError } from "./feedHealth";

/** What needs the reader's attention on a feed: an active site rate limit outranks the last fetch error. */
export type FeedAlert = { kind: "limited"; until: number } | { kind: "failed"; error: string };

export function feedAlert(feed: FeedView, now = Date.now()): FeedAlert | null {
  if (feed.limited_until !== null && feed.limited_until > now) return { kind: "limited", until: feed.limited_until };
  if (feed.error) return { kind: "failed", error: formatFeedError(feed.error) };
  return null;
}
