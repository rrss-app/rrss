import { useSyncExternalStore } from "react";
import { useTranslation } from "../../i18n";
import type { JumpController } from "./useFollowBottom";

/**
 * 「回到最新」：挂在输入区上方，离底部超过 120px 时出现。平时是圆键，悬停向左展开文字；
 * 不在底部时来了新内容，展开成强调色的「有新回复」。点按平滑滚到底并恢复跟随。
 */
export function JumpToLatest({ controller }: { controller: JumpController }) {
  const { t } = useTranslation("chat");
  const { away, fresh } = useSyncExternalStore(controller.subscribe, controller.get, controller.get);
  const label = fresh ? t("jump.fresh") : t("jump.latest");
  return (
    <button type="button" className="jump-latest" data-show={away ? "" : undefined} data-new={away && fresh ? "" : undefined}
      aria-label={label} inert={!away} onClick={controller.jump}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3.5 8.5 8 13l4.5-4.5" /></svg>
      <span className="jump-latest-label" aria-hidden="true">{label}</span>
    </button>
  );
}
