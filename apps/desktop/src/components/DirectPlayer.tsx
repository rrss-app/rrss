import { useEffect, useRef, useState } from "react";
import type { MediaRef } from "../bindings";
import { api } from "../lib/api";
import { useTranslation } from "../i18n";
import { isLocalMediaHref, siteLabel } from "./embedUrl";

const audioSeed = new URL("../assets/media/player-audio.wav?no-inline", import.meta.url).href;
const videoSeed = new URL("../assets/media/player-video.webm?no-inline", import.meta.url).href;

type Props = {
  url: string;
  blockUrl: string;
  articleId?: number;
  media: "video" | "audio";
  title: string;
  poster?: string;
  onMediaRef?: (ref: MediaRef) => void;
  onOpenUrl?: (href: string) => void;
};

function playableType(media: "video" | "audio", blobType: string): string {
  const type = blobType.split(";")[0]?.trim().toLowerCase() ?? "";
  if (type === "audio/vnd.wave" || type === "audio/wave" || type === "audio/x-wav") return "audio/wav";
  if (media === "audio" && ["video/mp4", "video/webm", "video/ogg"].includes(type)) return type.replace("video/", "audio/");
  if (media === "audio" && type.startsWith("audio/")) return type;
  if (media === "video" && (type.startsWith("video/") || type.startsWith("audio/"))) return type;
  return media === "audio" ? "audio/wav" : "video/mp4";
}

function firstFrame(video: HTMLVideoElement): string | undefined {
  if (!video.videoWidth || !video.videoHeight) return;
  const scale = Math.min(1, 320 / Math.max(video.videoWidth, video.videoHeight));
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
  canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
  const context = canvas.getContext("2d");
  if (!context) return;
  context.drawImage(video, 0, 0, canvas.width, canvas.height);
  const image = canvas.toDataURL("image/png");
  const prefix = "data:image/png;base64,";
  if (image.startsWith(prefix)) return image.slice(prefix.length);
}

export function DirectPlayer({ url, blockUrl, articleId, media, title, poster, onMediaRef, onOpenUrl }: Props) {
  const { t } = useTranslation("reader");
  const playerRef = useRef<HTMLMediaElement | null>(null);
  const frameRequested = useRef(false);
  const [src, setSrc] = useState("");
  const [external, setExternal] = useState(false);
  const [requested, setRequested] = useState(false);
  const [seedFailed, setSeedFailed] = useState(false);
  const busy = requested && !src && !external;
  const label = siteLabel(url);

  useEffect(() => {
    if (!requested || articleId == null) return;
    const controller = new AbortController();
    let objectUrl = "";
    void (async () => {
      try {
        const ref = await api.downloadDirect(articleId, blockUrl);
        if (controller.signal.aborted) return;
        if (!isLocalMediaHref(ref.href)) throw new Error("Invalid media reference");
        const response = await fetch(ref.href, { signal: controller.signal });
        if (!response.ok) throw new Error(String(response.status));
        const blob = await response.blob();
        const type = playableType(media, blob.type);
        const typed = blob.type.split(";")[0]?.trim().toLowerCase() === type
          ? blob
          : new Blob([await blob.arrayBuffer()], { type });
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(typed);
        setSrc(objectUrl);
      } catch {
        if (!controller.signal.aborted) setExternal(true);
      }
    })();
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [articleId, blockUrl, media, requested]);

  useEffect(() => {
    if (src) void playerRef.current?.play().catch(() => {});
  }, [src]);

  if (external || articleId == null) {
    return (
      <button type="button" className="embed-external" onClick={() => onOpenUrl?.(url)}>
        {external ? t("embed.playbackFallback", { site: label }) : label}
      </button>
    );
  }
  const playerProps = {
    ref: (element: HTMLMediaElement | null) => { playerRef.current = element; },
    className: "embed-player",
    "aria-label": title || label,
    controls: true,
    preload: src ? "auto" : "metadata",
    src: src || (media === "audio" ? audioSeed : videoSeed),
    onPlay: (event: React.SyntheticEvent<HTMLMediaElement>) => {
      if (src) return;
      const player = event.currentTarget;
      player.pause();
      player.load();
      setRequested(true);
    },
    onError: () => {
      if (src) setExternal(true);
      else if (!requested) setSeedFailed(true);
    },
  };
  return (
    <div aria-busy={busy}>
      {media === "audio" ? <audio {...playerProps} /> : (
        <video {...playerProps} playsInline poster={poster} onLoadedData={(event) => {
          if (!src || poster || frameRequested.current || !onMediaRef) return;
          frameRequested.current = true;
          try {
            const frame = firstFrame(event.currentTarget);
            if (frame) void api.cacheVideoFirstFrame(articleId, blockUrl, frame).then(onMediaRef).catch(() => {});
          } catch {
          }
        }} />
      )}
      {busy && <p role="status" className="embed-external">{t("embed.preparing")}</p>}
      {seedFailed && !requested && (
        <button type="button" className="embed-external" onClick={() => setRequested(true)}>{t("embed.play")}</button>
      )}
    </div>
  );
}
