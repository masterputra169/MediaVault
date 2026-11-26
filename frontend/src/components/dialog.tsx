"use client";

import { X } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";

export function Dialog({ title, children, onClose, locked, wide = false }: {
  title: string; children: ReactNode; onClose: () => void; locked?: boolean; wide?: boolean;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => { element?.close(); };
  }, []);

  return <dialog className={`panel modal ${wide ? "modal-wide" : ""}`} ref={dialog} aria-label={title} onCancel={(event) => {
    event.preventDefault();
    if (!locked) onClose();
  }}>
    <div className="modal-header">
      <h2>{title}</h2>
      <button type="button" className="button small modal-close-btn" disabled={locked} onClick={onClose} aria-label="Tutup dialog">
        <X size={15} /><span>Tutup</span>
      </button>
    </div>
    <div className="modal-body">
      {children}
    </div>
  </dialog>;
}
