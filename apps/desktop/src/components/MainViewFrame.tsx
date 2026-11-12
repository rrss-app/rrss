import { useRef, type ReactNode, type RefObject } from "react";
import { useRetainedScroll } from "../lib/useRetainedScroll";
import { useViewEnter } from "../lib/motion";

/** 到过才挂的主栏目外壳：非当前时 `hidden` + `inert`，界面与滚动位置留着。 */
export function MainViewFrame({ visible, rootRef, children }: {
  visible: boolean;
  rootRef?: RefObject<HTMLDivElement | null>;
  children: ReactNode;
}) {
  const ownRef = useRef<HTMLDivElement>(null);
  const ref = rootRef ?? ownRef;
  useRetainedScroll(ref, visible);
  // 切回这个主栏目：新内容淡入并上移 4px，旧内容不做退场。
  useViewEnter(ref, visible);
  return <div ref={ref} className="main-view" hidden={!visible} inert={!visible}>{children}</div>;
}
