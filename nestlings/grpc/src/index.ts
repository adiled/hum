// hum-grpc — gRPC bridge to hum's thrum.
//
// One service, one RPC: `Stream(stream Tone) returns (stream Tone)`.
// Every tone the daemon emits flows back through gRPC; every tone the
// client sends is forwarded to the daemon. Nothing gets translated —
// gRPC is the transport, thrum is still the protocol.
//
// Each gRPC bidi stream owns its own ThrumClient so concurrent clients
// can use overlapping sids without colliding handler maps.

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";

import { ThrumClient, type Tone } from "./thrum.ts";

const PORT = parseInt(process.env.HUM_GRPC_PORT ?? "14621", 10);
const HOST = process.env.HUM_GRPC_HOST ?? "0.0.0.0";

const PROTO_PATH = join(dirname(fileURLToPath(import.meta.url)), "..", "proto", "hum.proto");

interface WireTone {
  chi: string;
  sid: string;
  rid: string;
  body: Buffer | Uint8Array;
}

function toneToWire(msg: Tone): WireTone {
  return {
    chi: (msg.chi as string) ?? "",
    sid: (msg.sid as string) ?? "",
    rid: (msg.rid as string) ?? "",
    body: Buffer.from(JSON.stringify(msg), "utf8"),
  };
}

function wireToTone(w: WireTone): Tone {
  // Body is authoritative — it has the full tone shape. The routing fields
  // (chi/sid/rid) on the wire are just for the bridge; the daemon reads
  // them off the JSON body.
  try {
    const raw = Buffer.isBuffer(w.body) ? w.body : Buffer.from(w.body);
    const parsed = JSON.parse(raw.toString("utf8")) as Tone;
    if (w.chi && !parsed.chi) parsed.chi = w.chi;
    if (w.sid && !parsed.sid) parsed.sid = w.sid;
    if (w.rid && !parsed.rid) parsed.rid = w.rid;
    return parsed;
  } catch {
    return { chi: w.chi, sid: w.sid, rid: w.rid };
  }
}

async function main(): Promise<void> {
  const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
    keepCase: true,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
  });
  const proto = grpc.loadPackageDefinition(packageDefinition) as any;
  const HumService = proto.hum.Hum.service;

  const server = new grpc.Server();
  server.addService(HumService, {
    Stream: (call: grpc.ServerDuplexStream<WireTone, WireTone>) => {
      const thrum = new ThrumClient();
      const subscribedSids = new Set<string>();
      let closed = false;

      const writeWire = (msg: Tone) => {
        if (closed) return;
        try { call.write(toneToWire(msg)); } catch { closed = true; }
      };

      // Catch-all for tones that arrive without a sid (breath, echo, pulse).
      thrum.onAny(writeWire);

      const subscribe = (sid: string) => {
        if (!sid || subscribedSids.has(sid)) return;
        subscribedSids.add(sid);
        thrum.on(sid, writeWire);
      };

      const cleanup = () => {
        if (closed) return;
        closed = true;
        for (const sid of subscribedSids) thrum.off(sid);
        thrum.close();
      };

      thrum.connect().catch((err: Error) => {
        if (!closed) call.destroy(err);
        cleanup();
      });

      call.on("data", (w: WireTone) => {
        const tone = wireToTone(w);
        if (typeof tone.sid === "string" && tone.sid) subscribe(tone.sid);
        thrum.send(tone);
      });
      call.on("end", () => { cleanup(); try { call.end(); } catch { /* ignore */ } });
      call.on("cancelled", cleanup);
      call.on("error", cleanup);
    },
  });

  const bindAddress = `${HOST}:${PORT}`;
  await new Promise<void>((resolve, reject) => {
    server.bindAsync(bindAddress, grpc.ServerCredentials.createInsecure(), (err, port) => {
      if (err) return reject(err);
      // eslint-disable-next-line no-console
      console.log(`[hum-grpc] listening on grpc://${HOST}:${port}`);
      resolve();
    });
  });
}

main().catch(e => {
  // eslint-disable-next-line no-console
  console.error("[hum-grpc] startup failed:", e);
  process.exit(1);
});
