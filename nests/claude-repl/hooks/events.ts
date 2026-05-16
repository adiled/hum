import { openSync, closeSync, createReadStream } from "fs";
import { Readable } from "stream";
import { trace } from "../../../log.ts";

// ── FIFO event reader ──
//
// Open the FIFO non-blocking. The relay script writes one line per
// hook firing: "<event>\t<payload-json>". Buffer partials, dispatch
// on each \n.

export type HookEventHandler = (event: string, payloadRaw: string) => void;

export interface FifoReader {
  /** Close the underlying fd + Readable. Safe to call multiple times. */
  close: () => void;
}

export function openFifoReader(fifoPath: string, onLine: HookEventHandler): FifoReader {
  let fifoStream: Readable | null = null;
  let fifoBuf = "";
  // Hoist fdRw out of the try so cleanup can closeSync() it; the
  // Readable wrapper around the fd does not reliably close it on
  // destroy(), leading to a FIFO fd leak across many harness cycles.
  let fdRw: number | null = null;
  try {
    // node's fs.createReadStream on a FIFO blocks until writer opens —
    // workaround: open with O_RDWR so we're both reader and (dummy)
    // writer, then create a Readable from the fd.
    fdRw = openSync(fifoPath, "r+");
    fifoStream = createReadStream("", { fd: fdRw, encoding: "utf8" }) as unknown as Readable;
    fifoStream!.on("data", (chunk: string) => {
      fifoBuf += chunk;
      let nl: number;
      while ((nl = fifoBuf.indexOf("\n")) >= 0) {
        const line = fifoBuf.slice(0, nl);
        fifoBuf = fifoBuf.slice(nl + 1);
        dispatch(line);
      }
    });
    fifoStream!.on("error", (err: Error) => {
      trace("harness.fifo.error", { err: String(err) });
    });
  } catch (e) {
    trace("harness.fifo.open.failed", { err: String(e) });
  }

  function dispatch(line: string): void {
    if (!line) return;
    const tab = line.indexOf("\t");
    const event = tab >= 0 ? line.slice(0, tab) : line;
    const payloadRaw = tab >= 0 ? line.slice(tab + 1) : "";
    trace("harness.hook", { event, payloadLen: payloadRaw.length, payload: payloadRaw.slice(0, 4000) });
    onLine(event, payloadRaw);
  }

  return {
    close: () => {
      // Close the FIFO fd before AND after destroy() — destroy on the
      // Readable wrapper does not reliably propagate closeSync to the
      // underlying fd we opened with O_RDWR, leaking the fd otherwise.
      if (fdRw !== null) { try { closeSync(fdRw); } catch {} }
      try { fifoStream?.destroy?.(); } catch {}
      if (fdRw !== null) { try { closeSync(fdRw); } catch {} fdRw = null; }
    },
  };
}
