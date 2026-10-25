// The OpenAI-compatible API this hive's own bees call, at OPENAI_API_BASE.
//
// Its model list is read at start and every five minutes, and every 15
// seconds until it first answers. A model is a scent model when the
// hive's config lists it under `scents`, or, with no such list, when its
// id says embed; every other model is the nurse's. A request is tried
// again on a network failure, a 429 or a 5xx, up to three times, waiting
// as the answer's Retry-After says, at most ten seconds, else a growing
// half second. A streamed answer is read as server-sent events.

export const API_BASE = (process.env.OPENAI_API_BASE ?? "https://api.openai.com/v1").replace(/\/+$/, "");
const API_KEY = process.env.OPENAI_API_KEY ?? "";
const TRIES = 3;

function headers(): Record<string, string> {
  return { "Content-Type": "application/json", ...(API_KEY ? { Authorization: `Bearer ${API_KEY}` } : {}) };
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function waitOf(resp: Response | undefined, attempt: number): number {
  const after = Number(resp?.headers.get("retry-after"));
  if (Number.isFinite(after) && after > 0) return Math.min(after * 1000, 10_000);
  return 500 * 2 ** attempt;
}

export async function post(path: string, body: unknown, signal?: AbortSignal): Promise<Response> {
  let last: unknown;
  for (let attempt = 0; attempt <= TRIES; attempt++) {
    let resp: Response | undefined;
    try {
      resp = await fetch(`${API_BASE}${path}`, { method: "POST", headers: headers(), body: JSON.stringify(body), signal });
      if (resp.ok || (resp.status !== 429 && resp.status < 500)) return resp;
      last = new Error(`${resp.status}: ${(await resp.text().catch(() => "")).slice(0, 500)}`);
    } catch (e) {
      if (signal?.aborted) throw e;
      last = e;
    }
    if (attempt < TRIES) await sleep(waitOf(resp, attempt));
  }
  throw last instanceof Error ? last : new Error(String(last));
}

export async function modelIds(): Promise<string[] | undefined> {
  try {
    const resp = await fetch(`${API_BASE}/models`, { headers: headers() });
    if (!resp.ok) return undefined;
    const body = (await resp.json()) as { data?: Array<{ id?: unknown }> };
    return (body.data ?? []).map((d) => (typeof d?.id === "string" ? d.id : "")).filter((s) => s.length > 0);
  } catch {
    return undefined;
  }
}

export function split(ids: string[], scents?: string[]): { grown: string[]; scents: string[] } {
  const isScent = (id: string) => (scents ? scents.includes(id) : /embed/i.test(id));
  return { grown: ids.filter((id) => !isScent(id)), scents: ids.filter(isScent) };
}

export async function* events(resp: Response): AsyncGenerator<any> {
  if (!resp.body) return;
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (!line.startsWith("data:")) continue;
      const data = line.slice(5).trim();
      if (data === "[DONE]") return;
      try {
        yield JSON.parse(data);
      } catch {}
    }
  }
}

export function watchModels(scents: string[] | undefined, onChange: (m: { grown: string[]; scents: string[] }) => void): void {
  let known = "";
  let answered = false;
  const look = async () => {
    const ids = await modelIds();
    if (ids) {
      answered = true;
      const m = split(ids, scents);
      const said = JSON.stringify(m);
      if (said !== known) {
        known = said;
        onChange(m);
      }
    }
    setTimeout(look, answered ? 5 * 60_000 : 15_000).unref();
  };
  void look();
}
