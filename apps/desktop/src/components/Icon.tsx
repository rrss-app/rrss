import type { SVGProps } from "react";

// 24px 网格上的线性图标；只做装饰，可访问名称由按钮文本或 aria-label 提供。
const PATHS = {
  warning: "m12 3 10 18H2zM12 9v5M12 17v.5",
  lock: "M6 11h12v10H6zM8 11V7a4 4 0 0 1 8 0v4",
  search: "M10.5 4a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13zM15.3 15.3 20 20",
  refresh: "M20 11a8 8 0 0 0-14.7-3.6M4 13a8 8 0 0 0 14.7 3.6M4 4v4.5h4.5M20 20v-4.5h-4.5",
  checkAll: "m3 12.5 4 4L15 8m-4 8.5 1.5 1.5L21 9",
  settings: "M4 7h9m4 0h3M4 12h3m4 0h9M4 17h13m4 0h-1M13 5v4M7 10v4M17 15v4",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v6M12 7.5v.5",
  star: "m12 3.5 2.6 5.5 6 .8-4.4 4.2 1.1 6-5.3-2.9L6.7 20l1.1-6-4.4-4.2 6-.8z",
  quote: "M6.5 6h4.5v5.5H8.5c0 2 1 3.2 2.5 3.8V17c-3-.6-4.5-2.7-4.5-5.7zm8 0H19v5.5h-2.5c0 2 1 3.2 2.5 3.8V17c-3-.6-4.5-2.7-4.5-5.7z",
  chevronDown: "m6 9.5 6 6 6-6",
  chevronLeft: "m14.5 6-6 6 6 6",
  chevronRight: "m9.5 6 6 6-6 6",
  arrowLeft: "M19 12H5m6-6-6 6 6 6",
  arrowRight: "M5 12h14m-6-6 6 6-6 6",
  arrowUp: "M12 19V5m-6 6 6-6 6 6",
  copy: "M9 9h11v11H9zM15 9V4H4v11h5",
  external: "M14 4h6v6m0-6-9 9M18 13.5V19a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5.5",
  translate: "M4 5h9M8.5 3v2M11 5c-.6 3.4-2.9 6.4-6 8.2M6.5 9c1.2 2.2 3 3.9 5 5M12.5 20l3.9-9 3.9 9M14 16.7h4.8",
  sparkles: "M12 4l1.8 4.7L18.5 10.5l-4.7 1.8L12 17l-1.8-4.7L5.5 10.5l4.7-1.8zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8zM5 3l.7 1.8L7.5 5.5l-1.8.7L5 8l-.7-1.8L2.5 5.5l1.8-.7z",
  fileText: "M7 3h7l5 5v13H7zM14 3v5h5M10 12h5M10 16h5",
  close: "m6 6 12 12M18 6 6 18",
  plus: "M12 5v14M5 12h14",
  rss: "M5 18.5a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1zM4.5 11.5a8 8 0 0 1 8 8M4.5 5.5a14 14 0 0 1 14 14",
  folder: "M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2.5h9A1.5 1.5 0 0 1 21 9v8.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z",
  filter: "M4 5h16l-6.2 7.3V18l-3.6 2v-7.7z",
  memory: "M9 4a3 3 0 0 0-3 3v1.5A3 3 0 0 0 5 14a3 3 0 0 0 1.5 2.6V18a3 3 0 0 0 5.5 1.7V5.5A3 3 0 0 0 9 4zm6 0a3 3 0 0 1 3 3v1.5a3 3 0 0 1 1 5.5 3 3 0 0 1-1.5 2.6V18a3 3 0 0 1-5.5 1.7V5.5A3 3 0 0 1 15 4z",
  cpu: "M7 7h10v10H7zM10 10h4v4h-4zM9 3v4m6-4v4M9 17v4m6-4v4M3 9h4m-4 6h4m10-6h4m-4 6h4",
  terminal: "m5 7 5 5-5 5M12 18h7",
  globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c2.5 2.6 2.5 15.4 0 18M12 3c-2.5 2.6-2.5 15.4 0 18",
  activity: "M3 12h4l2.5-6 4 12L16 12h5",
  database: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4",
  moon: "M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z",
  monitor: "M3 5h18v11H3zM9 20h6M12 16v4",
  send: "M4 12 20 4l-4 16-4.5-6.5zM11.5 13.5 20 4",
  enter: "M18.5 5.5v7h-13m4-4-4 4 4 4",
  stop: "M6 6h12v12H6z",
  digest: "M4 5h16v14H4zM8 9h8M8 13h8M8 17h4",
  chat: "M4 5h16v11H9l-5 4z",
  focus: "M12 4.5a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15zM12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5zM12 2v2.5M12 19.5V22M2 12h2.5M19.5 12H22",
  hash: "M5 9h14M5 15h14M10 4 8 20M16 4l-2 16",
  layers: "m12 4 8 4.5-8 4.5-8-4.5zM4 13l8 4.5 8-4.5",
  eye: "M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12zM12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
  clock: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 7.5V12l3 2",
  inbox: "M4 5h16v14H4zM4 13h5l1.5 2.5h3L15 13h5",
  check: "m5 12.5 4.5 4.5L19 7.5",
  bookmark: "M6 4h12v16l-6-4-6 4z",
  list: "M5 6h14M5 12h14M5 18h14",
  pin: "M8 4h8l-1 6 3 3v2h-5v5l-1 1-1-1v-5H6v-2l3-3z",
  edit: "m4 20 4.5-1L19 8.5a2 2 0 0 0-3-3L5.5 16zM14.5 7l3 3",
  trash: "M4 7h16M9 7V4h6v3M6.5 7l1 13h9l1-13M10 11v6M14 11v6",
  download: "M12 4v11m-5-5 5 5 5-5M5 20h14",
  upload: "M12 15V4m-5 5 5-5 5 5M5 20h14",
  keyboard: "M3 7h18v10H3zM7 10.5h.5M11 10.5h.5M15 10.5h.5M7 14h10",
  spinner: "M12 3.5a8.5 8.5 0 1 0 8.5 8.5",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({
  name,
  size = 16,
  filled = false,
  className,
  ...rest
}: { name: IconName; size?: number; filled?: boolean } & SVGProps<SVGSVGElement>) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
      className={className ? `icon ${className}` : "icon"}
      data-icon={name}
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={filled ? 1.2 : 1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...rest}
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
