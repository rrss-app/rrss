import { useRef, useState, type ReactNode } from "react";
import { FloatingMenu, type MenuAnchor } from "./FloatingMenu";
import { Icon } from "./Icon";

/**
 * 下拉触发器 + 浮层菜单（搜索筛选胶囊的同一套）。不用原生 `<select>`：WebView2 / WebKitGTK 会弹系统样式的列表。
 * `children` 拿到 `close`，单选项点了就关，多选项点了留着。
 */
export function MenuButton({ label, face, className, title, disabled, active, menuClassName, children }: {
  label: string;
  face: ReactNode;
  className?: string;
  title?: string;
  disabled?: boolean;
  active?: boolean;
  menuClassName?: string;
  children: (close: () => void) => ReactNode;
}) {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  // 菜单开着时再点触发器：外部按下先关了菜单，这次点击不应再把它打开。
  const closedByTrigger = useRef(false);
  const open = () => {
    if (closedByTrigger.current) { closedByTrigger.current = false; return; }
    const rect = button.current?.getBoundingClientRect();
    if (rect) setAnchor({ x: rect.left, y: rect.bottom + 4 });
  };
  const close = () => setAnchor(null);
  return <>
    <button ref={button} type="button" className={className} title={title} disabled={disabled} data-on={active || undefined}
      aria-haspopup="menu" aria-expanded={!!anchor} aria-label={label}
      onPointerDown={() => { closedByTrigger.current = !!anchor; }} onClick={open}>{face}</button>
    {anchor ? (
      <FloatingMenu anchor={anchor} placement="below" label={label} className={`reader-context-menu menu-list${menuClassName ? ` ${menuClassName}` : ""}`}
        restoreFocusTo={button.current} onClose={close}>
        {children(close)}
      </FloatingMenu>
    ) : null}
  </>;
}

/** 菜单里的一项：勾号只在选中时出现，位置固定，文字不随勾选左右跳。 */
export function MenuCheck({ checked, radio, onSelect, title, children }: { checked: boolean; radio?: boolean; onSelect: () => void; title?: string; children: ReactNode }) {
  return <button type="button" role={radio ? "menuitemradio" : "menuitemcheckbox"} aria-checked={checked} title={title} onClick={onSelect}>
    <Icon name="check" size={13} className="menu-check" /><span className="menu-label">{children}</span>
  </button>;
}

export type MenuOption<T> = { value: T; label: string };

/** 单选下拉：替代原生 `<select>`。触发器显示当前项文字，可选前置图标。 */
export function MenuSelect<T extends string | number>({ value, options, onChange, label, className = "menu-select", menuClassName, icon, prefix, disabled, title, active }: {
  value: T;
  options: readonly MenuOption<T>[];
  onChange: (value: T) => void;
  label: string;
  className?: string;
  menuClassName?: string;
  icon?: ReactNode;
  /** 触发器上当前值前面的固定文字，如「时间：」。 */
  prefix?: string;
  disabled?: boolean;
  title?: string;
  active?: boolean;
}) {
  const current = options.find((option) => option.value === value);
  return <MenuButton label={label} className={className} menuClassName={menuClassName} title={title} disabled={disabled} active={active}
    face={<>{icon}<span className="menu-select-value">{prefix}{current?.label ?? ""}</span><Icon name="chevronDown" size={12} className="menu-caret" /></>}>
    {(close) => options.map((option) => (
      <MenuCheck key={String(option.value)} radio checked={option.value === value} onSelect={() => { onChange(option.value); close(); }}>{option.label}</MenuCheck>
    ))}
  </MenuButton>;
}
