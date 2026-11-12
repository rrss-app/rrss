export function ThumbIcon({ down = false }: { down?: boolean }) {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
        strokeLinecap="round"
        transform={down ? "rotate(180 8 8)" : undefined}
      >
        <path d="M6.25 13.5h5.05c.68 0 1.2-.52 1.28-1.16l.62-4.2A1.05 1.05 0 0 0 12.16 7H9V4.1C9 3.2 8.4 2.5 7.55 2.5c-.38 0-.7.22-.82.56L5.7 7H4.2c-.58 0-1.05.47-1.05 1.05v4.4c0 .58.47 1.05 1.05 1.05h2.05z" />
      </g>
    </svg>
  );
}
