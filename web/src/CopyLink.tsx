import { useRef, useState } from "react";

/**
 * the clipboard api only exists on https and localhost; over plain http inside the
 * network, fall back to selecting the text and the older copy command
 */
export async function writeClipboard(text: string, element: HTMLElement | null) {
  if (navigator.clipboard) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // denied; try the fallback
    }
  }
  if (!element) return false;
  const range = document.createRange();
  range.selectNodeContents(element);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  const copied = document.execCommand("copy");
  selection?.removeAllRanges();
  return copied;
}

/** copies a link that opens the same thing for whoever it is sent to */
export function CopyLink({ link, label = "copy link" }: { link: string; label?: string }) {
  const [copied, setCopied] = useState<boolean | null>(null);
  const text = useRef<HTMLSpanElement>(null);

  const copy = async () => {
    setCopied(await writeClipboard(link, text.current));
    setTimeout(() => setCopied(null), 1500);
  };

  return (
    <button
      onClick={copy}
      title={link}
      aria-label={label}
      className="h-7 rounded-[4px] border border-line px-2.5 text-[13px] text-fog-300 hover:border-fog-700 hover:text-fog-100"
    >
      {copied === null ? label : copied ? "copied" : "copy failed"}
      <span ref={text} className="sr-only">
        {link}
      </span>
    </button>
  );
}
