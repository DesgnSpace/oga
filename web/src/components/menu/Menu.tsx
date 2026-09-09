// The compact, native-feeling menu shared by the task header's "more actions"
// trigger and the sidebar row context menu. One presentational component,
// different items per caller.

import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export interface MenuAction {
  key: string;
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
  destructive?: boolean;
  disabled?: boolean;
}

export interface MenuPanelProps {
  sections: MenuAction[][];
  prompt?: ReactNode;
  className?: string;
  onClose?: () => void;
}

export function MenuPanel({ sections, prompt, className, onClose }: MenuPanelProps) {
  const visible = sections.filter((section) => section.length > 0);
  const items = visible.flat();
  const enabledItems = items.filter((item) => !item.disabled);
  const [focusedKey, setFocusedKey] = useState(enabledItems[0]?.key);
  const itemRefs = useRef(new Map<string, HTMLButtonElement>());
  const openerRef = useRef<HTMLElement | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    openerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const firstItem = enabledItems[0];
    if (firstItem) {
      itemRefs.current.get(firstItem.key)?.focus();
    }

    const closeOnPointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (menuRef.current?.contains(target) || openerRef.current?.contains(target))) return;
      onClose?.();
    };
    document.addEventListener("pointerdown", closeOnPointer);
    return () => document.removeEventListener("pointerdown", closeOnPointer);
  }, []);

  const focusItem = (item: MenuAction) => {
    setFocusedKey(item.key);
    itemRefs.current.get(item.key)?.focus();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const currentIndex = enabledItems.findIndex((item) => item.key === focusedKey);
    let nextIndex: number | undefined;
    if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % enabledItems.length;
    if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + enabledItems.length) % enabledItems.length;
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = enabledItems.length - 1;

    if (nextIndex !== undefined && enabledItems.length > 0) {
      event.preventDefault();
      focusItem(enabledItems[nextIndex]);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      onClose?.();
      openerRef.current?.focus();
      return;
    }
    if (event.key === "Tab") {
      onClose?.();
      return;
    }
    if ((event.key === "Enter" || event.key === " ") && currentIndex >= 0) {
      event.preventDefault();
      itemRefs.current.get(enabledItems[currentIndex].key)?.click();
    }
  };

  return (
    <div
      ref={menuRef}
      className={`menu-panel${className ? ` ${className}` : ""}`}
      role="menu"
      onKeyDown={handleKeyDown}
    >
      {prompt && <p className="menu-prompt">{prompt}</p>}
      {visible.map((section, index) => (
        <div className="menu-section" key={section[0]?.key ?? index}>
          {index > 0 && <div className="menu-separator" role="separator" />}
          {section.map((item) => (
            <button
              key={item.key}
              type="button"
              role="menuitem"
              className={`menu-item${item.destructive ? " menu-item-destructive" : ""}`}
              disabled={item.disabled}
              tabIndex={item.key === focusedKey && !item.disabled ? 0 : -1}
              ref={(element) => {
                if (element) itemRefs.current.set(item.key, element);
                else itemRefs.current.delete(item.key);
              }}
              onFocus={() => setFocusedKey(item.key)}
              onClick={item.onSelect}
            >
              {item.icon && (
                <span className="menu-item-icon" aria-hidden="true">
                  {item.icon}
                </span>
              )}
              <span className="menu-item-label">{item.label}</span>
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
