import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useTranslation } from "../i18n";
import { isLocalMediaHref, siteLabel } from "./embedUrl";

type Props = {
  url: string;
  articleId?: number;
  media: "video" | "audio";
  onOpenUrl?: (href: string) => void;
};

function playableType(media: "video" | "audio", blobType: string): string {
  const type = blobType.split(";")[0]?.trim().toLowerCase() ?? "";
  if (type === "audio/vnd.wave" || type === "audio/wave" || type === "audio/x-wav") return "audio/wav";
  if (media === "audio" && type.startsWith("audio/")) return type;
  if (media === "video" && (type.startsWith("video/") || type.startsWith("audio/"))) return type;
  return media === "audio" ? "audio/wav" : "video/mp4";
}

export function DirectPlayer({ url, articleId, media, onOpenUrl }: Props) {
  const { t } = useTranslation("reader");
  const [href, setHref] = useState("");
  const [src, setSrc] = useState("");
  const [external, setExternal] = useState(false);
  const [busy, setBusy] = useState(false);
  const label = siteLabel(url);

  useEffect(() => {
    if (!href) return;
    let cancelled = false;
    let objectUrl = "";
    void (async () => {
      try {
        const response = await fetch(href);
        if (!response.ok) throw new Error(String(response.status));
        const blob = await response.blob();
        const type = playableType(media, blob.type);
        const typed = blob.type.split(";")[0]?.trim().toLowerCase() === type
          ? blob
          : new Blob([await blob.arrayBuffer()], { type });
        objectUrl = URL.createObjectURL(typed);
        if (!cancelled) setSrc(objectUrl);
      } catch {
        if (!cancelled) setExternal(true);
      }
    })();
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [href, media]);

  if (external || articleId == null) {
    return (
      <button type="button" className="embed-external" onClick={() => onOpenUrl?.(url)}>
        {label}
      </button>
    );
  }
  if (src) {
    const player = media === "audio"
      ? <audio className="embed-player" controls preload="metadata" src={src} onError={() => setExternal(true)} />
      : <video className="embed-player" controls preload="metadata" src={src} onError={() => setExternal(true)} />;
    return player;
  }
  if (href && isLocalMediaHref(href)) {
    return <p className="embed-external">{t("embed.preparing")}</p>;
  }
  return (
    <button
      type="button"
      className="embed-external"
      disabled={busy}
      onClick={() => {
        setBusy(true);
        void api.downloadDirect(articleId, url).then((ref) => {
          if (isLocalMediaHref(ref.href)) setHref(ref.href);
          else setExternal(true);
        }).catch(() => setExternal(true)).finally(() => setBusy(false));
      }}
    >
      {busy ? t("embed.downloading") : t("embed.play")}
    </button>
  );
}
