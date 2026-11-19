import { useContext } from "react";
import type { EmbedCoverView, MediaRef } from "../bindings";
import { WebLinksContext } from "../lib/webLinks";
import { EmbedCoverCard } from "./EmbedCoverCard";
import { DirectPlayer } from "./DirectPlayer";
import { classifyEmbed, isLocalMediaHref } from "./embedUrl";

type EmbedBlock = { kind: string; url: string; title: string; poster?: string };

type Props = {
  block: EmbedBlock;
  covers?: EmbedCoverView[];
  articleId?: number;
  mediaOf: (src: string) => MediaRef | undefined;
  onMediaRef?: (ref: MediaRef) => void;
  onOpenUrl?: (href: string) => void;
};

export function EmbedBlock({ block, covers = [], articleId, mediaOf, onMediaRef, onOpenUrl }: Props) {
  const links = useContext(WebLinksContext);
  const openInBrowser = (url: string) => {
    if (links) links.open(url, "", "browser");
    else onOpenUrl?.(url);
  };
  const classified = classifyEmbed(block.url, block.kind);
  switch (classified.kind) {
    case "youtube":
    case "bilibili": {
      const cover = covers.find((item) => item.url === classified.openUrl || item.open_url === classified.openUrl || item.url === block.url);
      return (
        <EmbedCoverCard
          cover={cover}
          site={classified.kind}
          openUrl={classified.openUrl}
          onOpenUrl={openInBrowser}
        />
      );
    }
    case "video":
    case "audio": {
      const poster = classified.kind === "video"
        ? [block.poster, `rrss-frame:${block.url}`]
            .map((src) => src && mediaOf(src)?.href)
            .find((href) => href && isLocalMediaHref(href))
        : undefined;
      return (
        <DirectPlayer
          key={`${articleId}:${classified.openUrl}`}
          url={classified.openUrl}
          blockUrl={block.url}
          articleId={articleId}
          media={classified.kind}
          title={block.title}
          poster={poster}
          onMediaRef={onMediaRef}
          onOpenUrl={openInBrowser}
        />
      );
    }
    case "external": {
      const usable = block.title && !block.title.includes("://") && !block.title.includes("?") && !block.title.includes(block.url);
      return (
        <button type="button" className="embed-external" onClick={() => openInBrowser(classified.openUrl)}>
          {usable ? block.title : classified.label}
        </button>
      );
    }
  }
}
