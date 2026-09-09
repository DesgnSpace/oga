// House icons — one visual language for the whole desktop app.
// Outline only, 16-unit grid, stroke 1.5, round caps/joins, currentColor,
// slightly taller than wide (panel 12×14 centred in 16×16), rx≈2.
//
// Geometry rule — never mix families on the same surface. A circle-outline
// info icon next to a rounded-square file icon looks mismatched because the
// silhouettes do not share a container. Pick one container per concept and
// keep neighbours as siblings:
//   • Round (circle outline, r≈5.2) for status / info / alerts — Info,
//     Question, Exclamation, Cancel, Blocked, and toast variants.
//   • Rounded-square (12×14, rx≈2) for objects — panels, files, windows,
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

function Svg({ size = 16, className, style, children, viewBox = "0 0 16 16" }: IconProps & { children: React.ReactNode; viewBox?: string }) {
  return (
    <svg
      viewBox={viewBox}
      width={size}
      height={size}
      className={className}
      style={style}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
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
      <rect x={2} y={1} width={12} height={14} rx={2} />
      <path d="M6 1v14" />
    </Svg>
  );
}

export function BackArrowIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M10 3.5 5 8l5 4.5" />
    </Svg>
  );
}

export function ForwardArrowIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M6 3.5 11 8 6 12.5" />
    </Svg>
  );
}

export function InfoIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.2} />
      <path d="M8 7.2v3.8" />
      <circle cx={8} cy={5} r={0.9} fill="currentColor" stroke="none" />
    </Svg>
  );
}


export function RefreshIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M13 8A5 5 0 1 1 11.1 3.4M13 2.5v3h-3" />
    </Svg>
  );
}

export function SearchIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={7} cy={7} r={4} />
      <path d="M10.5 10.5 13.5 13.5" />
    </Svg>
  );
}

export function CloseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4 4 12 12M12 4 4 12" />
    </Svg>
  );
}

export function FilterIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M2.5 4.5h11M4.5 8h7M6.5 11.5h3" />
    </Svg>
  );
}

export function SettingsIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M13.8 6.8V9.2l-1.7.4-.6 1.1 1 1.5-.85.85-1.5-1-.1-.05-1.1.6-.4 1.7H7.2l-.4-1.7-1.1-.6-1.5 1-.85-.85 1-1.5-.6-1.1-1.7-.4V6.8l1.7-.4.6-1.1-1-1.5.85-.85 1.5 1 1.1-.6.4-1.7H8.8l.4 1.7 1.1.6 1.5-1 .85.85-1 1.5.6 1.1z" />
      <circle cx={8} cy={8} r={1.9} />
    </Svg>
  );
}

export function UsageIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3 13V8M8 13V4M13 13V6" />
    </Svg>
  );
}

// -- task / menu -----------------------------------------------------------

export function ArchiveIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2.5} y={4} width={11} height={2.2} rx={1} />
      <rect x={3.8} y={6.2} width={8.4} height={6.8} rx={1} />
      <path d="M6.5 9.2h3" />
    </Svg>
  );
}

export function RestoreIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={2.5} y={4} width={11} height={2.2} rx={1} />
      <rect x={3.8} y={6.2} width={8.4} height={6.8} rx={1} />
      <path d="M8 11V7M5.9 9.1 8 7l2.1 2.1" />
    </Svg>
  );
}

export function CheckIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3 8.2 6 11.5 13 4.5" />
    </Svg>
  );
}

export function PauseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M5.5 3.5v9M10.5 3.5v9" />
    </Svg>
  );
}

export function PlayIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M5 3.5 11.8 8 5 12.5z" />
    </Svg>
  );
}

export function CancelIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5} />
      <path d="M5 5 11 11" />
    </Svg>
  );
}

export function MoreIcon({ size = 16, className }: IconProps) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      className={className}
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx={8} cy={3.5} r={1.35} />
      <circle cx={8} cy={8} r={1.35} />
      <circle cx={8} cy={12.5} r={1.35} />
    </svg>
  );
}

export function ChevronIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M6 3.5 11 8 6 12.5" />
    </Svg>
  );
}

export function DisclosureIcon({ open, size = 16, className }: IconProps & { open: boolean }) {
  return (
    <Svg size={size} className={className}>
      <path d={open ? "M3.5 8h9" : "M3.5 8h9M8 3.5v9"} />
    </Svg>
  );
}

export function DiffMarkIcon({ kind, size = 16, className }: IconProps & { kind: "added" | "removed" }) {
  return (
    <Svg size={size} className={className}>
      <path d={kind === "added" ? "M8 3.5v9M3.5 8h9" : "M3.5 8h9"} />
    </Svg>
  );
}

export function QuestionMarkIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.2} />
      <path d="M5.8 6a2.5 2.5 0 0 1 3.8 2c-.8.55-1.35 1-1.35 1.9" />
      <circle cx={8} cy={12.2} r={0.9} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function ExclamationIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M8 4.5V9.5" />
      <circle cx={8} cy={12.2} r={0.9} fill="currentColor" stroke="none" />
      <circle cx={8} cy={8} r={5.2} />
    </Svg>
  );
}

export function BlockedIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <circle cx={8} cy={8} r={5.2} />
      <rect x={6} y={6} width={4} height={4} rx={0.8} fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function ChangedFilesIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4 2h5.2L12 4.8V14H4z" />
      <path d="M6.5 9.5h3M8 8v3" />
    </Svg>
  );
}

export function AttachmentIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4 2h5.2L12 4.8V14H4z" />
      <path d="M6 6.5h4M6 9h4M6 11.5h2.5" />
    </Svg>
  );
}

export function ScopeIcon({ size = 16, className }: IconProps) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      className={className}
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx={8} cy={8} r={3.2} />
    </svg>
  );
}

export function FollowUpIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M4 4.5 7 8 4 11.5M8 4.5 11 8 8 11.5" />
    </Svg>
  );
}

export function ReplyIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M10 5 6 8l4 3M6 8h4a1.8 1.8 0 0 1 0 3.6H9" />
    </Svg>
  );
}

export function SteerIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 5.5h6M8.5 3 11 5.5 8.5 8M12.5 10.5H6M7 13 4.5 10.5 7 8" />
    </Svg>
  );
}

export function HandoffIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3 10.5V8A1.5 1.5 0 0 1 4.5 6.5H7M11 8.5 13.5 6 11 3.5M4.5 8h9" />
    </Svg>
  );
}

export function ResponseIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M3.5 8.2 6.2 11 12.5 4.8" />
    </Svg>
  );
}

export function CopyIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <rect x={6.5} y={6.5} width={6} height={6} rx={1} />
      <rect x={3.5} y={3.5} width={6} height={6} rx={1} />
    </Svg>
  );
}

export function SendIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M8 12.5V3.5M4 7 8 3 12 7" />
    </Svg>
  );
}

export function CodeIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M11 3.5 7.5 8 11 12.5M5 3.5 8.5 8 5 12.5" />
    </Svg>
  );
}

export function ExternalLinkIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      {/* rounded square missing top-right corner, arrow leaves through gap */}
      <path d="M5 2H8.6M12.2 6.2V10.5M10.2 12.5H5M3 10.5V4M3 4A2 2 0 0 1 5 2M10.2 12.5A2 2 0 0 1 12.2 10.5M5 12.5A2 2 0 0 1 3 10.5M3 4A2 2 0 0 1 5 2" />
      <path d="M7.8 7 13 1.8M9.2 1.8H13v3.8" />
    </Svg>
  );
}

export function OpenExternalIcon({ size = 16, className }: IconProps) {
  return (
    <Svg size={size} className={className}>
      <path d="M5 2H8.6M12.2 6.2V10.5A2 2 0 0 1 10.2 12.5H5A2 2 0 0 1 3 10.5V4A2 2 0 0 1 5 2" />
      <path d="M7.8 7 13 1.8M9.2 1.8H13v3.8" />
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
