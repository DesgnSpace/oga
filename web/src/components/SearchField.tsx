import { useRef } from "react";
import type { KeyboardEventHandler, Ref } from "react";
import { CloseIcon, SearchIcon } from "@/ui/icons";

type SearchFieldProps = {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  className?: string;
  inputClassName?: string;
  inputRef?: Ref<HTMLInputElement>;
  onKeyDown?: KeyboardEventHandler<HTMLInputElement>;
  "aria-label"?: string;
  title?: string;
  "data-task-search"?: boolean;
};

export function SearchField({ value, onChange, placeholder, className, inputClassName, inputRef, onKeyDown, "aria-label": ariaLabel, title, "data-task-search": dataTaskSearch }: SearchFieldProps) {
  const internalInputRef = useRef<HTMLInputElement>(null);
  const setInputRef = (node: HTMLInputElement | null) => {
    internalInputRef.current = node;
    if (typeof inputRef === "function") inputRef(node);
    else if (inputRef) inputRef.current = node;
  };

  return (
    <div className={`search-field${className ? ` ${className}` : ""}`}>
      <span className="search-field-icon">
        <SearchIcon />
      </span>
      <input
        ref={setInputRef}
        className={inputClassName}
        type="search"
        spellCheck={false}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            if (value !== "") onChange("");
            else internalInputRef.current?.blur();
            return;
          }
          onKeyDown?.(event);
        }}
        placeholder={placeholder}
        aria-label={ariaLabel ?? placeholder}
        title={title}
        data-task-search={dataTaskSearch}
      />
      {value !== "" && (
        <button
          className="icon-button search-field-clear"
          type="button"
          aria-label="Clear search"
          title="Clear search"
          onClick={() => {
            onChange("");
            internalInputRef.current?.focus();
          }}
        >
          <CloseIcon />
        </button>
      )}
    </div>
  );
}
