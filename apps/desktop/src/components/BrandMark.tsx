import { BRAND_GLYPH } from "./brandMarkGlyph";

/** 品牌标「R。」：墨色方块（.brand-mark）里的 R 用 --on-ink，句号用 --brand-dot，跟随亮暗主题。 */
export function BrandMark() {
  const { transform, d, stroke, dot } = BRAND_GLYPH;
  return (
    <span className="brand-mark" aria-hidden="true">
      <svg viewBox="0 0 1024 1024" focusable="false">
        <g transform={transform}>
          <path d={d} fill="currentColor" stroke="currentColor" strokeWidth={stroke} strokeLinejoin="round" />
          <circle cx={dot.cx} cy={dot.cy} r={dot.r} fill="var(--brand-dot)" />
        </g>
      </svg>
    </span>
  );
}
