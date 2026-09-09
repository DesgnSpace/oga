import { CodeIcon } from "@/ui/icons";

export function CodeGlyph({ size = 14, className = "line-glyph" }: { size?: number; className?: string }) {
  return <CodeIcon size={size} className={className} />;
}
