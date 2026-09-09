// Chips for the files or images handed to a worker alongside a request,
// shown beside the request bubble. Images preview in place, reusing the
// same disk-read and modal pattern as the trace's file preview; other
// files open in the OS's own app for that file type.

import * as React from "react";
import { openAttachment, readImagePreview } from "@/bridge";
import { Modal } from "@/components/primitives/Modal";
import { AttachmentIcon } from "@/ui/icons";
import { resolvePreviewPath } from "./Trace";

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

function isImagePath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  return dot >= 0 && IMAGE_EXTENSIONS.has(path.slice(dot + 1).toLowerCase());
}

function fileName(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.slice(normalized.lastIndexOf("/") + 1) || path;
}

function useDiskImage(path: string) {
  const [src, setSrc] = React.useState<string>();
  const [error, setError] = React.useState<string>();
  React.useEffect(() => {
    let active = true;
    void readImagePreview(path).then((result) => {
      if (!active) return;
      if (!result.ok) {
        setError(result.error.message);
        return;
      }
      const bytes = new Uint8Array(result.value.bytes);
      let binary = "";
      for (let offset = 0; offset < bytes.length; offset += 0x8000) {
        binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
      }
      setSrc(`data:${result.value.mime};base64,${btoa(binary)}`);
    });
    return () => {
      active = false;
    };
  }, [path]);
  return { src, error };
}

function AttachmentImage({ path }: { path: string }) {
  const [open, setOpen] = React.useState(false);
  const { src, error } = useDiskImage(path);
  return (
    <>
      <button type="button" className="attachment-chip attachment-chip-image" title={fileName(path)} onClick={() => setOpen(true)}>
        {src ? <img src={src} alt={fileName(path)} /> : <span className="attachment-chip-status" aria-hidden="true" />}
      </button>
      <Modal open={open} onClose={() => setOpen(false)} labelledBy="attachment-preview-title" className="modal-dialog-file-preview">
        <h2 id="attachment-preview-title" className="trace-file-preview-modal-title">
          {fileName(path)}
        </h2>
        {error ? (
          <p className="trace-file-preview-error" role="status">{error}</p>
        ) : src ? (
          <img className="trace-file-preview-modal-image" src={src} alt={fileName(path)} />
        ) : (
          <p className="trace-file-preview-status" role="status">Loading image…</p>
        )}
      </Modal>
    </>
  );
}

function AttachmentFile({ path }: { path: string }) {
  return (
    <button
      type="button"
      className="attachment-chip attachment-chip-file"
      title={path}
      onClick={() => void openAttachment(path)}
    >
      <AttachmentIcon size={14} />
      <span className="attachment-chip-name">{fileName(path)}</span>
    </button>
  );
}

/** What the worker got alongside the prompt, next to the request that sent it. */
export function AttachmentsRow({ paths, cwd }: { paths: string[] | undefined; cwd?: string }) {
  if (!paths || paths.length === 0) return null;
  return (
    <div className="attachments-row" aria-label={paths.length === 1 ? "1 file attached" : `${paths.length} files attached`}>
      {paths.map((path) => {
        const resolved = resolvePreviewPath(path, cwd);
        return isImagePath(path) ? (
          <AttachmentImage path={resolved} key={path} />
        ) : (
          <AttachmentFile path={resolved} key={path} />
        );
      })}
    </div>
  );
}
