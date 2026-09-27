// File and image attachments beside a request.

import * as React from "react";
import { openAttachment, readTaskAttachment } from "@/bridge";
import { Modal } from "@/components/primitives/Modal";
import { AttachmentIcon, ExclamationIcon } from "@/ui/icons";

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

function isImagePath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  return dot >= 0 && IMAGE_EXTENSIONS.has(path.slice(dot + 1).toLowerCase());
}

function fileName(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.slice(normalized.lastIndexOf("/") + 1) || path;
}

/** Names one attachment by its place in the task's list; the broker resolves the file. */
interface AttachmentRef {
  taskId: string;
  index: number;
  path: string;
}

function useAttachmentImage({ taskId, index }: AttachmentRef) {
  const [src, setSrc] = React.useState<string>();
  const [error, setError] = React.useState<string>();
  React.useEffect(() => {
    let active = true;
    void readTaskAttachment(taskId, index).then((result) => {
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
  }, [taskId, index]);
  return { src, error };
}

function AttachmentImage({ attachment }: { attachment: AttachmentRef }) {
  const { path } = attachment;
  const titleId = React.useId();
  const [open, setOpen] = React.useState(false);
  const { src, error } = useAttachmentImage(attachment);
  return (
    <>
      <button type="button" className="attachment-chip attachment-chip-image" aria-label={fileName(path)} title={error ? "Couldn't load preview" : fileName(path)} onClick={() => setOpen(true)}>
        {src ? <img src={src} alt={fileName(path)} /> : error ? <ExclamationIcon size={16} /> : <span className="attachment-chip-status" aria-hidden="true" />}
      </button>
      <Modal open={open} onClose={() => setOpen(false)} labelledBy={titleId} className="modal-dialog-file-preview">
        <h2 id={titleId} className="trace-file-preview-modal-title">
          {fileName(path)}
        </h2>
        {error ? (
          <p className="trace-file-preview-error" role="alert">{error}</p>
        ) : src ? (
          <img className="trace-file-preview-modal-image" src={src} alt={fileName(path)} />
        ) : (
          <p className="trace-file-preview-status" role="status">Loading image…</p>
        )}
      </Modal>
    </>
  );
}

function AttachmentFile({ attachment: { taskId, index, path } }: { attachment: AttachmentRef }) {
  return (
    <button
      type="button"
      className="attachment-chip attachment-chip-file"
      title={path}
      onClick={() => void openAttachment(taskId, index)}
    >
      <AttachmentIcon size={14} />
      <span className="attachment-chip-name">{fileName(path)}</span>
    </button>
  );
}

/** The files the delegating caller attached for whoever follows the task. */
export function AttachmentsRow({ taskId, paths }: { taskId: string; paths: string[] | undefined }) {
  if (!paths || paths.length === 0) return null;
  return (
    <div className="attachments-row" aria-label={paths.length === 1 ? "1 file attached" : `${paths.length} files attached`}>
      {paths.map((path, index) => {
        const attachment = { taskId, index, path };
        return isImagePath(path) ? (
          <AttachmentImage attachment={attachment} key={`${index}:${path}`} />
        ) : (
          <AttachmentFile attachment={attachment} key={`${index}:${path}`} />
        );
      })}
    </div>
  );
}
