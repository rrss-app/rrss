import type { EmbedCoverView } from "../bindings";
import { coverMeta, isLocalMediaHref } from "./embedUrl";

type Props = {
  cover?: EmbedCoverView;
  site: "youtube" | "bilibili";
  openUrl: string;
  onOpenUrl?: (href: string) => void;
};

export function EmbedCoverCard({ cover, site, openUrl, onOpenUrl }: Props) {
  const ready = Boolean(cover && !cover.failed && cover.title && isLocalMediaHref(cover.cover_href));
  const label = site === "bilibili" ? "哔哩哔哩" : "YouTube";
  if (!ready || !cover) {
    const text = cover?.failed ? `${label} · 封面未能显示` : label;
    return (
      <button type="button" className="embed-external" onClick={() => onOpenUrl?.(openUrl)}>
        {text}
      </button>
    );
  }
  const meta = coverMeta(site, cover.author, cover.duration_secs);
  return (
    <button type="button" className="embed-cover" onClick={() => onOpenUrl?.(cover.open_url || openUrl)}>
      <span className="embed-cover-frame">
        <img src={cover.cover_href} alt="" />
        <span className="embed-cover-play" aria-hidden="true" />
      </span>
      <span className="embed-cover-body">
        <span className="embed-cover-title">{cover.title}</span>
        <span className="embed-cover-meta">{meta}</span>
      </span>
    </button>
  );
}
