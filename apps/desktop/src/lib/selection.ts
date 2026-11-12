export type SelectedBlock = { blockId: number; excerpt: string };
export type SelectionPick = { excerpt: string; blockId: number; blocks: SelectedBlock[] };

/** 限定在一个文档内，逐块裁剪 Range；来源按钮、列表标记不属于选区正文。 */
export function readSelection(root?: HTMLElement | null): SelectionPick | null {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return null;
  const anchor = selection.anchorNode instanceof Element ? selection.anchorNode : selection.anchorNode?.parentElement;
  const scope = root ?? anchor?.closest<HTMLElement>("[data-reader-document]");
  if (!scope) return null;
  const range = selection.getRangeAt(0);
  if (!scope.contains(range.startContainer) || !scope.contains(range.endContainer)) return null;
  const blocks: SelectedBlock[] = [];
  for (const element of scope.querySelectorAll<HTMLElement>("[data-block-id]")) {
    if (!range.intersectsNode(element)) continue;
    const blockId = Number(element.dataset.blockId);
    if (!Number.isSafeInteger(blockId) || blockId < 0) continue;
    const boundary = document.createRange();
    boundary.selectNodeContents(element);
    const part = range.cloneRange();
    if (part.compareBoundaryPoints(Range.START_TO_START, boundary) < 0) {
      part.setStart(boundary.startContainer, boundary.startOffset);
    }
    if (part.compareBoundaryPoints(Range.END_TO_END, boundary) > 0) {
      part.setEnd(boundary.endContainer, boundary.endOffset);
    }
    if (part.collapsed) continue;
    const content = part.cloneContents();
    content.querySelectorAll("button, [data-selection-ignore]").forEach((node) => node.remove());
    content.querySelectorAll("br").forEach((node) => node.replaceWith("\n"));
    const excerpt = content.textContent?.trim() ?? "";
    if (excerpt) blocks.push({ blockId, excerpt });
  }
  if (!blocks.length) return null;
  const anchorId = Number(anchor?.closest<HTMLElement>("[data-block-id]")?.dataset.blockId);
  return {
    excerpt: blocks.map((block) => block.excerpt).join("\n"),
    blockId: blocks.some((block) => block.blockId === anchorId) ? anchorId : blocks[0].blockId,
    blocks,
  };
}
