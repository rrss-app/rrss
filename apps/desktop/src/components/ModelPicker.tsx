import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { fuzzyMatch, type FuzzyHit } from "../lib/fuzzy";
import { usePresence } from "../lib/motion";
import { Icon } from "./Icon";

export type PickerItem = {
  /** 选中后回传的值。 */
  key: string;
  /** 完整模型 id，或「跟随主模型」这类文字项。 */
  label: string;
  /** 文字项不用等宽字体。 */
  plain?: boolean;
  /** 连接小标签与悬停提示。 */
  tag?: string;
  tagTitle?: string;
  /** 第二行说明。 */
  hint?: string;
  /** 标签前的附加标记（「不能调用工具」「默认」）。 */
  marks?: ReactNode;
  /** 查询为空时按它分组。 */
  group?: string;
  /** 对话中可就地改显示名：当前显示名与留空时显示的缩写。 */
  rename?: { value: string; placeholder: string };
};

export type ModelPickerProps = {
  /** 触发按钮与弹层的无障碍名称。 */
  label: string;
  value: string | null;
  items: PickerItem[];
  onPick: (key: string) => void;
  /** 按钮上的文字（缩写）与悬停提示（完整 id 与连接）。 */
  display: { text: string; title: string; plain?: boolean };
  /** 按钮上名字后的附加文字，例如对话里的思考档位。 */
  badge?: ReactNode;
  /** 列表下面的附加设置（对话里的思考强度）。 */
  extra?: ReactNode;
  heading?: { title: string; hint?: string };
  placeholder?: string;
  /** 列表下方的说明与入口，例如模型池为空时。 */
  notice?: ReactNode;
  /** 查询没有命中时的补充（例如去模型池添加）。 */
  noMatch?: (query: string) => ReactNode;
  /** 查询不是现有候选时的手填入口。 */
  manual?: (query: string, close: () => void) => ReactNode;
  footer?: (close: () => void) => ReactNode;
  /** `chip` 用在对话输入区：无边框、只显示缩写。 */
  variant?: "field" | "chip";
  placement?: "down" | "up";
  disabled?: boolean;
  /** `warn`：还没选模型，按钮用提示色。 */
  tone?: "warn";
  /** 保存就地改名（`item.rename`）：成功返回 null，失败返回原因；菜单不关。 */
  onRename?: (key: string, name: string) => Promise<string | null>;
};

/** 显示名最多 32 个字符。 */
const NAME_MAX = 32;
type Editing = { key: string; text: string; error: string; saving: boolean };

type Row = { item: PickerItem; hit: FuzzyHit };

/**
 * 模型选择列表：设置按钮显示缩写与连接，对话按钮只显示缩写；展开后是完整 id 加连接，当前项打勾。顶部可输入模糊搜索，
 * 回车选中高亮项（默认第一项），上下键移动，Esc 关闭并把焦点还给按钮。各用途、对话默认与对话中共用。
 */
export function ModelPicker(props: ModelPickerProps) {
  const { label, value, items, onPick, display, variant = "field", placement = "down", disabled } = props;
  const [open, setOpen] = useState(false);
  // 收起时菜单留 100ms 播退场（不可交互、对读屏隐藏）。
  const presence = usePresence(open, 100);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [editing, setEditing] = useState<Editing | null>(null);
  const editingRef = useRef(editing);
  editingRef.current = editing;

  const selected = items.find((item) => item.key === value);
  const q = query.trim();
  const rows = useMemo<Row[]>(() => {
    const matched = items.flatMap((item) => {
      const hit = fuzzyMatch(item.label, q)
      ?? ((item.tag && fuzzyMatch(item.tag, q)) || (item.hint && fuzzyMatch(item.hint, q)) ? { score: 0, indices: [] } : null);
      return hit ? [{ item, hit }] : [];
    });
    return q ? matched.sort((a, b) => b.hit.score - a.hit.score) : matched;
  }, [items, q]);

  const close = (refocus = true) => {
    // 搜索词在下次打开时清空：退场期间列表不变。
    setOpen(false);
    setEditing(null);
    if (refocus) trigger.current?.focus();
  };
  const pick = (key: string) => {
    onPick(key);
    close();
  };

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) close(false);
    };
    // 即使输入焦点已移到菜单外，Esc 仍先关闭模型浮层。
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing || event.keyCode === 229 || root.current?.closest("[inert], [hidden]")) return;
      event.preventDefault(); event.stopImmediatePropagation();
      // 改名中 Esc 只取消改名，菜单留着。
      const edit = editingRef.current;
      if (edit) { if (!edit.saving) { setEditing(null); focusRename(edit.key); } return; }
      close();
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", escape, true);
    };
  }, [open]);

  // 靠窗口边缘时整体平移，窄窗口里不撑出横向滚动。
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const node = menu.current;
      if (!node) return;
      node.style.transform = "";
      const rect = node.getBoundingClientRect();
      const edge = 8;
      const shift = rect.right > window.innerWidth - edge ? window.innerWidth - edge - rect.right
        : rect.left < edge ? edge - rect.left : 0;
      if (shift) node.style.transform = `translateX(${Math.round(shift)}px)`;
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [open]);

  const focusRename = (key: string) => requestAnimationFrame(() =>
    root.current?.querySelector<HTMLButtonElement>(`[data-rename-for="${CSS.escape(key)}"]`)?.focus());
  const saveRename = async () => {
    const edit = editingRef.current;
    if (!edit || edit.saving || !props.onRename) return;
    const name = edit.text.trim();
    if (name === items.find((i) => i.key === edit.key)?.rename?.value) { setEditing(null); focusRename(edit.key); return; }
    setEditing({ ...edit, saving: true, error: "" });
    const error = await props.onRename(edit.key, name);
    setEditing((cur) => (cur?.key !== edit.key ? cur : error ? { ...cur, saving: false, error } : null));
    if (!error) focusRename(edit.key);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    // 「改名」按钮上的回车是点它自己，不是选中高亮行。
    if ((e.target as HTMLElement).closest(".model-picker-rename")) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (rows.length ? (i + step + rows.length) % rows.length : 0));
    } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      const row = rows[Math.min(active, rows.length - 1)];
      if (row) pick(row.item.key);
    }
  };

  const grouped = !q && items.some((i) => i.group);
  let lastGroup: string | undefined;
  const optionId = (i: number) => `${listId}-${i}`;

  return (
    <div className="model-picker" data-variant={variant} ref={root} onKeyDown={open ? onKey : undefined}>
      <button type="button" ref={trigger} className="model-picker-trigger" aria-label={label} title={display.title} data-tone={props.tone}
        aria-haspopup="dialog" aria-expanded={open} disabled={disabled}
        onClick={() => { setActive(0); setOpen((o) => !o); setQuery(""); }}>
        {props.badge
          // 等宽的模型名和界面字的标注上沿不同：两段放进一个按基线排的组，组本身仍在按钮里居中。
          ? <span className="model-picker-label"><span className={display.plain ? "model-picker-text" : "model-picker-text mono"}>{display.text}</span>{props.badge}</span>
          : <span className={display.plain ? "model-picker-text" : "model-picker-text mono"}>{display.text}</span>}
        {variant === "field" && selected?.tag && <> <span className="model-picker-connection">{selected.tag}</span></>}
        <Icon name="chevronDown" size={12} />
      </button>
      {presence.mounted && (
        <div className={`model-picker-menu ${placement}`} ref={menu} role="dialog" aria-label={label}
          data-leaving={presence.leaving ? "" : undefined} inert={presence.leaving} aria-hidden={presence.leaving || undefined}>
          {props.heading && (
            <div className="model-picker-head"><strong>{props.heading.title}</strong>{props.heading.hint && <small>{props.heading.hint}</small>}</div>
          )}
          {items.length > 1 && (
            <div className="model-picker-search">
              <input type="search" autoFocus value={query} placeholder={props.placeholder ?? "搜索模型"} aria-label="搜索模型"
                role="combobox" aria-expanded="true" aria-controls={listId} autoComplete="off"
                aria-activedescendant={rows.length ? optionId(Math.min(active, rows.length - 1)) : undefined}
                onChange={(e) => { setQuery(e.target.value); setActive(0); }} />
            </div>
          )}
          {items.length > 0 && <ul className="model-picker-list" role="listbox" id={listId} aria-label={label}>
            {rows.map(({ item, hit }, i) => {
              const heading = grouped && item.group !== lastGroup ? item.group : undefined;
              lastGroup = item.group;
              const option = (
                <button type="button" role="option" id={optionId(i)} aria-selected={item.key === value}
                  data-active={i === Math.min(active, rows.length - 1) ? "true" : undefined}
                  title={item.tag ? `${item.hint ?? item.label} · ${item.tagTitle ?? item.tag}` : undefined}
                  onMouseEnter={() => setActive(i)} onClick={() => pick(item.key)}>
                  <Icon name="check" size={14} className="model-picker-check" />
                  <span className="model-picker-name">
                    <span className={item.plain ? undefined : "mono"}>{highlight(item.label, hit.indices)}</span>
                    {item.hint && <small>{item.hint}</small>}
                  </span>
                  {item.marks}
                  {item.tag && <span className="model-picker-tag" title={item.tagTitle}>{item.tag}</span>}
                </button>
              );
              const rename = props.onRename ? item.rename : undefined;
              const edit = editing?.key === item.key ? editing : null;
              return (
                <li key={item.key} role="presentation">
                  {heading && <div className="model-picker-group" role="presentation">{heading}</div>}
                  {!rename ? option : edit ? (
                    <div className="model-picker-rename-edit">
                      {/* 写入中只读而不禁用：禁用会丢焦点，失焦又等同取消。 */}
                      <input autoFocus value={edit.text} maxLength={NAME_MAX} readOnly={edit.saving} aria-busy={edit.saving}
                        placeholder={rename.placeholder} aria-label={`「${item.hint ?? item.label}」的显示名`} aria-invalid={!!edit.error}
                        onFocus={(e) => e.currentTarget.select()}
                        onChange={(e) => setEditing({ ...edit, text: e.target.value, error: "" })}
                        onBlur={() => { if (!editingRef.current?.saving) setEditing(null); }}
                        onKeyDown={(e) => {
                          e.stopPropagation();
                          if (e.key === "Enter" && !e.nativeEvent.isComposing && e.keyCode !== 229) { e.preventDefault(); void saveRename(); }
                        }} />
                      <small className={edit.error ? "form-status error" : "hint"} role={edit.error ? "alert" : undefined}>
                        {edit.error || `回车保存，留空用缩写 · ${item.hint ?? item.label}`}
                      </small>
                    </div>
                  ) : (
                    <div className="model-picker-row">
                      {option}
                      <button type="button" className="model-picker-rename" data-rename-for={item.key} aria-label={`改名「${item.label}」`}
                        onClick={() => setEditing({ key: item.key, text: rename.value, error: "", saving: false })}>改名</button>
                    </div>
                  )}
                </li>
              );
            })}
            {q && !rows.length && <li className="model-picker-none" role="presentation">没有匹配「{q}」的模型。{props.noMatch?.(q)}</li>}
          </ul>}
          {props.notice && <div className="model-picker-empty">{props.notice}</div>}
          {props.extra}
          {q && props.manual && !items.some((i) => i.label === q) && <div className="model-picker-manual">{props.manual(q, () => close())}</div>}
          {props.footer && <div className="model-picker-foot">{props.footer(() => close(false))}</div>}
        </div>
      )}
    </div>
  );
}

function highlight(text: string, indices: number[]) {
  if (!indices.length) return text;
  const marked = new Set(indices);
  return [...text].map((ch, i) => (marked.has(i) ? <mark key={i}>{ch}</mark> : ch));
}
