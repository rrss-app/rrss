import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { Icon } from "../components/Icon";

export function ModelField({ label, value, onChange, placeholder, options, disabled }: {
  label: string;
  value: string;
  onChange: (val: string) => void;
  placeholder: string;
  options: string[];
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const [placement, setPlacement] = useState<CSSProperties>({ left: 0, width: 0 });
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const listId = useId();

  const filtered = filter ? options.filter((option) => option.toLowerCase().includes(filter.toLowerCase())) : options;
  const activeIndex = Math.min(active, Math.max(0, filtered.length - 1));

  // fixed 不受内容滚动区裁切；菜单仍在设置的焦点边界内。
  const openMenu = (opener: HTMLElement) => {
    if (disabled || options.length === 0) return;
    const rect = wrapRef.current?.getBoundingClientRect();
    if (!rect) return;
    const drawer = wrapRef.current?.closest(".settings-drawer")?.getBoundingClientRect();
    const leftEdge = Math.max(8, (drawer?.left ?? 0) + 8);
    const rightEdge = Math.min(window.innerWidth - 8, (drawer?.right ?? window.innerWidth) - 8);
    const width = Math.min(rect.width, Math.max(0, rightEdge - leftEdge));
    const below = window.innerHeight - rect.bottom - 12;
    const above = rect.top - 12;
    const flip = below < 256 && above > below;
    setPlacement({
      ...(flip ? { bottom: window.innerHeight - rect.top + 4 } : { top: rect.bottom + 4 }),
      left: Math.max(leftEdge, Math.min(rect.left, rightEdge - width)),
      width,
      maxHeight: Math.max(0, Math.min(256, flip ? above : below)),
    });
    openerRef.current = opener;
    setFilter("");
    setActive(Math.max(0, options.indexOf(value)));
    setOpen(true);
  };

  const close = (restoreFocus: boolean) => {
    setOpen(false);
    if (restoreFocus) openerRef.current?.focus({ preventScroll: true });
  };

  const pick = (option: string) => {
    onChange(option);
    close(true);
  };

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (wrapRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    const onScroll = (event: Event) => {
      if (menuRef.current?.contains(event.target as Node)) return;
      if (menuRef.current?.contains(document.activeElement)) openerRef.current?.focus({ preventScroll: true });
      setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
    };
  }, [open]);

  useEffect(() => {
    if (disabled || options.length === 0) setOpen(false);
  }, [disabled, options.length]);

  useLayoutEffect(() => {
    if (open) searchRef.current?.focus({ preventScroll: true });
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const list = listRef.current;
    const option = list?.querySelector<HTMLElement>('[data-index="' + activeIndex + '"]');
    if (!list || !option) return;
    // scrollIntoView 会滚动外层抽屉并触发关闭；只移动候选列表自身。
    if (option.offsetTop < list.scrollTop) list.scrollTop = option.offsetTop;
    else if (option.offsetTop + option.offsetHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = option.offsetTop + option.offsetHeight - list.clientHeight;
    }
  }, [open, activeIndex, filter]);

  const onListKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive(Math.max(0, Math.min(activeIndex + 1, filtered.length - 1)));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive(Math.max(activeIndex - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (filtered[activeIndex]) pick(filtered[activeIndex]);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    }
  };

  return (
    <div className="field">
      <span>{label}</span>
      <div className="model-input-wrap" ref={wrapRef}>
        <input
          value={value}
          aria-label={label}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
          placeholder={placeholder}
          className="model-text-input"
          onKeyDown={(event) => {
            if (!event.nativeEvent.isComposing && event.key === "ArrowDown" && options.length > 0 && !open) {
              event.preventDefault();
              openMenu(event.currentTarget);
            }
          }}
        />
        {options.length > 0 && (
          <button
            type="button"
            className="model-dropdown-trigger"
            disabled={disabled}
            aria-label={label + "：选择模型"}
            aria-haspopup="listbox"
            aria-expanded={open}
            aria-controls={open ? listId : undefined}
            onClick={(event) => (open ? close(false) : openMenu(event.currentTarget))}
            title="从已获取的模型列表中选择"
          >
            <span>{value ? "切换" : "选择"}</span>
            <Icon name="chevronDown" size={12} />
          </button>
        )}
        {open && !disabled && options.length > 0 && (
          <div ref={menuRef} className="model-dropdown-menu" data-scroll-boundary style={placement} onKeyDown={onListKeyDown}
            onBlur={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
            }}>
            <div className="model-dropdown-search">
              <input
                ref={searchRef}
                role="combobox"
                aria-autocomplete="list"
                aria-expanded="true"
                value={filter}
                onChange={(event) => { setFilter(event.target.value); setActive(0); }}
                placeholder="搜索模型…"
                aria-label="搜索模型"
                aria-controls={listId}
                aria-activedescendant={filtered[activeIndex] ? listId + "-" + activeIndex : undefined}
              />
            </div>
            <div className="model-dropdown-list" role="listbox" id={listId} aria-label={label + "候选"} ref={listRef}>
              {filtered.length === 0 ? (
                <div className="model-dropdown-empty" role="status">无匹配模型</div>
              ) : filtered.map((option, index) => (
                <button
                  key={option}
                  id={listId + "-" + index}
                  type="button"
                  role="option"
                  aria-selected={option === value}
                  data-index={index}
                  data-active={index === activeIndex ? "1" : "0"}
                  data-selected={option === value ? "1" : "0"}
                  tabIndex={-1}
                  className="model-dropdown-item"
                  title={option}
                  onMouseMove={() => setActive(index)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => pick(option)}
                >
                  <span>{option}</span>
                  {option === value && <Icon name="check" size={13} className="model-item-check" />}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
