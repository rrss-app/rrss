import type { EmbedCoverView } from "../bindings";
import { EmbedCoverCard } from "./EmbedCoverCard";
import { DirectPlayer } from "./DirectPlayer";
import { classifyEmbed } from "./embedUrl";

type EmbedBlock = { kind: string; url: string; title: string };

type Props = {
  block: EmbedBlock;
  covers?: EmbedCoverView[];
  articleId?: number;
  onOpenUrl?: (href: string) => void;
};

export function EmbedBlock({ block, covers = [], articleId, onOpenUrl }: Props) {
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
          onOpenUrl={onOpenUrl}
        />
      );
    }
    case "video":
    case "audio":
      return (
        <DirectPlayer
          url={classified.openUrl}
          articleId={articleId}
          media={classified.kind}
          onOpenUrl={onOpenUrl}
        />
      );
    case "external": {
      const usable = block.title && !block.title.includes("://") && !block.title.includes("?") && !block.title.includes(block.url);
      return (
        <button type="button" className="embed-external" onClick={() => onOpenUrl?.(classified.openUrl)}>
          {usable ? block.title : classified.label}
        </button>
      );
    }
  }
}
