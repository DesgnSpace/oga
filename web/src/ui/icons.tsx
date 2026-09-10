// House icons — one visual language for the whole desktop app.
// Outline only, 16-unit grid, stroke 1.25, round caps/joins, currentColor.
// Every glyph sits in a 12×12 optical box centred in the 16×16 viewBox and
// never crosses x/y 2 or 14. Rounded-square containers use rx≈2.5.
//
// Geometry rule — never mix families on the same surface. A circle-outline
// info icon next to a rounded-square file icon looks mismatched because the
// silhouettes do not share a container. Pick one container per concept and
// keep neighbours as siblings:
//   • Round (circle outline, r=5.5) for status / info / alerts — Info,
//     Question, Exclamation, Cancel, Blocked, and toast variants.
//   • Rounded-square (12×12, rx≈2.5) for objects — panels, files, windows,
//     copy/duplicate boxes, external-link squares, archive boxes.
// Actions and navigation (arrows, chevrons, search, refresh) stay as minimal
// strokes with no container so they do not compete with either family.

import * as React from "react";
import type { CSSProperties } from "react";

type IconProps = {
  size?: number;
  className?: string;
  style?: CSSProperties;
};

function Svg({ size = 16, className, style, children }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      className={className}
      style={style}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.25}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}


export function SidebarIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2} y={2} width={12} height={12} rx={2.5} />
      <path d="M6 2v12" />
    </Svg>
  );
}

export function BackArrowIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M10 3 6 8l4 5" />
    </Svg>
  );
}

export function ForwardArrowIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M6 3l4 5-4 5" />
    </Svg>
  );
}

export function InfoIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.5} />
      <path d="M8 7.8V11" />
      <circle cx={8} cy={5.8} r={0.75} fill="currentColor" stroke="none" />
    </Svg>
  );
}


export function RefreshIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M13.4 8A5.4 5.4 0 1 1 11.5 3.9M13.4 2.7v2.8h-2.8" />
    </Svg>
  );
}

export function SearchIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={7} cy={7} r={4.2} />
      <path d="M10 10 13.2 13.2" />
    </Svg>
  );
}

export function CloseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4.5 4.5 11.5 11.5M11.5 4.5 4.5 11.5" />
    </Svg>
  );
}

export function FilterIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M2.8 4.5h10.4M5 8h6M7 11.5h2" />
    </Svg>
  );
}

export function SettingsIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={3.5} />
      <path d="M11.5 8h1.8M10.48 10.48l1.27 1.27M8 11.5v1.8M5.52 10.48 4.25 11.75M4.5 8H2.7M5.52 5.52 4.25 4.25M8 4.5V2.7M10.48 5.52l1.27-1.27" />
      <circle cx={8} cy={8} r={1.5} />
    </Svg>
  );
}

export function UsageIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 13V8M8 13V3M12.5 13V9.5" />
    </Svg>
  );
}

// -- task / menu -----------------------------------------------------------

export function ArchiveIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2.5} y={3.5} width={11} height={2.6} rx={1.3} />
      <rect x={4} y={6.1} width={8} height={6.4} rx={1.5} />
      <path d="M6.5 8.5h3" />
    </Svg>
  );
}

export function RestoreIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2.5} y={3.5} width={11} height={2.6} rx={1.3} />
      <rect x={4} y={6.1} width={8} height={6.4} rx={1.5} />
      <path d="M8 11.4V7.9M6.4 9.5 8 7.9l1.6 1.6" />
    </Svg>
  );
}

export function CheckIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 8.5 6.8 11.8 12.5 4.2" />
    </Svg>
  );
}

export function PauseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M5.8 3v10M10.2 3v10" />
    </Svg>
  );
}

export function PlayIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4.8 3 11.2 8 4.8 13Z" />
    </Svg>
  );
}

export function CancelIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.5} />
      <path d="M6 6 10 10M10 6 6 10" />
    </Svg>
  );
}

export function MoreIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={4} r={1.2} fill="currentColor" stroke="none" />
      <circle cx={8} cy={8} r={1.2} fill="currentColor" stroke="none" />
      <circle cx={8} cy={12} r={1.2} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function ChevronIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M6 3l4 5-4 5" />
    </Svg>
  );
}

export function DisclosureIcon({ open, size = 16, className }: IconProps & { open: boolean }) {
  return (
    <Svg size={size} className={className}>
      <path d={open ? "M3.8 8h8.4" : "M3.8 8h8.4M8 3.8v8.4"} />
    </Svg>
  );
}

export function DiffMarkIcon({ kind, size = 16, className }: IconProps & { kind: "added" | "removed" }) {
  return (
    <Svg size={size} className={className}>
      <path d={kind === "added" ? "M8 3.8v8.4M3.8 8h8.4" : "M3.8 8h8.4"} />
    </Svg>
  );
}

export function QuestionMarkIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.5} />
      <path d="M6.4 6.2A2.2 2.2 0 0 1 9.9 7.7c-.75.5-1.6.95-1.6 2.1" />
      <circle cx={8} cy={11.7} r={0.7} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function ExclamationIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.5} />
      <path d="M8 5.2V9.6" />
      <circle cx={8} cy={11.6} r={0.75} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function BlockedIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.5} />
      <rect x={6.2} y={6.2} width={3.6} height={3.6} rx={1} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function ChangedFilesIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 2.5H9.2L12.5 5.8V13.5H3.5Z" />
      <path d="M8 8.4v2.8M6.6 9.8h2.8" />
    </Svg>
  );
}

export function AttachmentIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 2.5H9.2L12.5 5.8V13.5H3.5Z" />
      <path d="M6 7.5h4M6 10.5h4" />
    </Svg>
  );
}

export function TerminalIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2} y={2} width={12} height={12} rx={2.5} />
      <path d="M5.8 6.3 7.8 8.3 5.8 10.3M9.3 10.3h2.1" />
    </Svg>
  );
}

export function ScopeIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={3.2} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function FollowUpIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4.5 4 7.5 8 4.5 12M8.5 4 11.5 8 8.5 12" />
    </Svg>
  );
}

export function ReplyIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M6.05 10.93 2.8 7.68 6.05 4.43" />
      <path d="M13.2 11.58V10.28A2.6 2.6 0 0 0 10.6 7.68H2.8" />
    </Svg>
  );
}

export function SteerIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 5.5h7M8.2 3.2 10.5 5.5 8.2 7.8M12.5 10.5h-7M7.8 8.2 5.5 10.5 7.8 12.8" />
    </Svg>
  );
}

export function HandoffIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4 11.2V9.5A2.5 2.5 0 0 1 6.5 7H12M9.8 4.8 12 7 9.8 9.2" />
    </Svg>
  );
}

export function ResponseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 8.3 6.3 11 12.5 4.8" />
    </Svg>
  );
}

export function CopyIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={3} y={3} width={7.2} height={7.2} rx={1.8} />
      <rect x={5.8} y={5.8} width={7.2} height={7.2} rx={1.8} />
    </Svg>
  );
}

export function SendIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M8 12.5V3.8M4.6 7.2 8 3.8 11.4 7.2" />
    </Svg>
  );
}

export function CodeIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M10.5 4 6.5 8 10.5 12M5.5 4 9.5 8 5.5 12" />
    </Svg>
  );
}

export function ExternalLinkIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M9.4 5H4.5A1.5 1.5 0 0 0 3 6.5V11.5A1.5 1.5 0 0 0 4.5 13H10A1.5 1.5 0 0 0 11.5 11.5V6.4" />
      <path d="M8.4 7.6 13 3M9.6 3H13v3.4" />
    </Svg>
  );
}

export function OpenExternalIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={3} y={5} width={8} height={8} rx={1.8} />
      <path d="M8.6 7.4 13 3M9.7 3H13v3.4" />
    </Svg>
  );
}

// Keep alias for CodeGlyph compatibility
export const CodeGlyph = CodeIcon;

// Toast-semantic aliases — same strokes, callers pick by intent.
export const ToastPendingIcon = RefreshIcon;
export const ToastSuccessIcon = CheckIcon;
export const ToastErrorIcon = ExclamationIcon;
export const ToastInfoIcon = InfoIcon;

// All icon names for gallery iteration
type GalleryIconProps = IconProps & { open?: boolean; kind?: "added" | "removed" };
export const iconRegistry: Array<{ name: string; Component: React.ComponentType<GalleryIconProps> }> = [
  { name: "SidebarIcon", Component: SidebarIcon },
  { name: "BackArrowIcon", Component: BackArrowIcon },
  { name: "ForwardArrowIcon", Component: ForwardArrowIcon },
  { name: "InfoIcon", Component: InfoIcon },
  { name: "RefreshIcon", Component: RefreshIcon },
  { name: "SearchIcon", Component: SearchIcon },
  { name: "CloseIcon", Component: CloseIcon },
  { name: "FilterIcon", Component: FilterIcon },
  { name: "SettingsIcon", Component: SettingsIcon },
  { name: "ArchiveIcon", Component: ArchiveIcon },
  { name: "RestoreIcon", Component: RestoreIcon },
  { name: "CheckIcon", Component: CheckIcon },
  { name: "PauseIcon", Component: PauseIcon },
  { name: "PlayIcon", Component: PlayIcon },
  { name: "CancelIcon", Component: CancelIcon },
  { name: "MoreIcon", Component: MoreIcon },
  { name: "ChevronIcon", Component: ChevronIcon },
  { name: "BlockedIcon", Component: BlockedIcon },
  { name: "ChangedFilesIcon", Component: ChangedFilesIcon },
  { name: "TerminalIcon", Component: TerminalIcon },
  { name: "ScopeIcon", Component: ScopeIcon },
  { name: "FollowUpIcon", Component: FollowUpIcon },
  { name: "ReplyIcon", Component: ReplyIcon },
  { name: "SteerIcon", Component: SteerIcon },
  { name: "HandoffIcon", Component: HandoffIcon },
  { name: "ResponseIcon", Component: ResponseIcon },
  { name: "CopyIcon", Component: CopyIcon },
  { name: "SendIcon", Component: SendIcon },
  { name: "CodeIcon", Component: CodeIcon },
  { name: "ExternalLinkIcon", Component: ExternalLinkIcon },
  { name: "OpenExternalIcon", Component: OpenExternalIcon },
  { name: "QuestionMarkIcon", Component: QuestionMarkIcon },
  { name: "ExclamationIcon", Component: ExclamationIcon },
];
