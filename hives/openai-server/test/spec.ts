// What OpenAI's spec and the Open Responses spec say, as checks.
//
// The specs are the vendored ones in spec/ (spec/fetch.ts). A wire is
// checked by its route: a request body against the spec its client wrote
// to, OpenAI's unless said, since the two ask different things of a
// request (Open Responses a `type` on every message, OpenAI a `detail` on
// every image and `strict` on every function); a JSON answer against the
// schema its status names; and each event of a stream against the event
// schema of its route, a Responses event against both specs by its `type`.
//
// An `mcp_call` item is OpenAI's, and OpenAI's clients read it, so it is
// kept, though Open Responses wants an item it does not define named with
// an implementor's prefix; against Open Responses it is set aside.

import Ajv2020 from "ajv/dist/2020.js";
import openai from "../spec/openai.json" with { type: "json" };
import openresponses from "../spec/openresponses.json" with { type: "json" };

const ajv = new Ajv2020({ strict: false, validateFormats: false, allErrors: true });
ajv.addSchema(openai as object, "openai");
ajv.addSchema(openresponses as object, "openresponses");

type Spec = "openai" | "openresponses";

const pointer = (...parts: string[]) => parts.map((p) => encodeURIComponent(p.replace(/~/g, "~0").replace(/\//g, "~1"))).join("/");

function against(spec: Spec, ref: string, value: unknown): string[] {
  const validate = ajv.getSchema(`${spec}#${ref}`);
  if (!validate) return [`${spec} has no ${decodeURIComponent(ref)}`];
  if (validate(value)) return [];
  return (validate.errors ?? []).map((e) => `${spec} ${decodeURIComponent(ref)}: ${e.instancePath || "/"} ${e.message}${e.params ? ` ${JSON.stringify(e.params)}` : ""}`);
}

export function schema(spec: Spec, name: string, value: unknown): string[] {
  return against(spec, `/components/schemas/${pointer(name)}`, value);
}

const ROUTES: Array<[RegExp, string]> = [
  [/^\/chat\/completions$/, "/chat/completions"],
  [/^\/completions$/, "/completions"],
  [/^\/embeddings$/, "/embeddings"],
  [/^\/models$/, "/models"],
  [/^\/models\/[^/]+$/, "/models/{model}"],
  [/^\/responses$/, "/responses"],
  [/^\/responses\/[^/]+$/, "/responses/{response_id}"],
];

export function routeOf(url: string): string | undefined {
  const path = new URL(url).pathname.replace(/^\/v1/, "");
  return ROUTES.find(([re]) => re.test(path))?.[1];
}

export function asked(route: string, method: string, body: unknown, spec: Spec = "openai"): string[] {
  const op = `/paths/${pointer(route, method.toLowerCase())}`;
  if (spec === "openresponses") return route === "/responses" ? schema("openresponses", "CreateResponseBody", body) : [];
  return against("openai", `${op}/requestBody/content/${pointer("application/json")}/schema`, body);
}

const OPENAI_ONLY_ITEMS = new Set(["mcp_call"]);

function openOnly(value: unknown): unknown {
  if (!value || typeof value !== "object") return value;
  const v = value as Record<string, unknown>;
  const response = v.response as Record<string, unknown> | undefined;
  if (Array.isArray(v.output)) return { ...v, output: v.output.filter((i) => !OPENAI_ONLY_ITEMS.has((i as { type?: string })?.type ?? "")) };
  if (response && Array.isArray(response.output)) return { ...v, response: openOnly(response) };
  return v;
}

const openAside = (event: Record<string, unknown>) => OPENAI_ONLY_ITEMS.has(String((event.item as { type?: string } | undefined)?.type));

export function answered(route: string, method: string, status: number, body: unknown): string[] {
  const at = status >= 400 ? "ErrorResponse" : undefined;
  if (at) return schema("openai", at, body);
  const findings = against("openai", `/paths/${pointer(route, method.toLowerCase(), "responses", String(status), "content", "application/json", "schema")}`, body);
  if (route.startsWith("/responses") && method.toUpperCase() !== "DELETE") findings.push(...schema("openresponses", "ResponseResource", openOnly(body)));
  return findings;
}

const OPEN_EVENTS: Record<string, string> = {
  "response.created": "ResponseCreatedStreamingEvent",
  "response.queued": "ResponseQueuedStreamingEvent",
  "response.in_progress": "ResponseInProgressStreamingEvent",
  "response.completed": "ResponseCompletedStreamingEvent",
  "response.failed": "ResponseFailedStreamingEvent",
  "response.incomplete": "ResponseIncompleteStreamingEvent",
  "response.output_item.added": "ResponseOutputItemAddedStreamingEvent",
  "response.output_item.done": "ResponseOutputItemDoneStreamingEvent",
  "response.content_part.added": "ResponseContentPartAddedStreamingEvent",
  "response.content_part.done": "ResponseContentPartDoneStreamingEvent",
  "response.output_text.delta": "ResponseOutputTextDeltaStreamingEvent",
  "response.output_text.done": "ResponseOutputTextDoneStreamingEvent",
  "response.output_text.annotation.added": "ResponseOutputTextAnnotationAddedStreamingEvent",
  "response.refusal.delta": "ResponseRefusalDeltaStreamingEvent",
  "response.refusal.done": "ResponseRefusalDoneStreamingEvent",
  "response.reasoning.delta": "ResponseReasoningDeltaStreamingEvent",
  "response.reasoning.done": "ResponseReasoningDoneStreamingEvent",
  "response.reasoning_summary_part.added": "ResponseReasoningSummaryPartAddedStreamingEvent",
  "response.reasoning_summary_part.done": "ResponseReasoningSummaryPartDoneStreamingEvent",
  "response.reasoning_summary_text.delta": "ResponseReasoningSummaryDeltaStreamingEvent",
  "response.reasoning_summary_text.done": "ResponseReasoningSummaryDoneStreamingEvent",
  "response.function_call_arguments.delta": "ResponseFunctionCallArgumentsDeltaStreamingEvent",
  "response.function_call_arguments.done": "ResponseFunctionCallArgumentsDoneStreamingEvent",
  error: "ErrorStreamingEvent",
};

const schemas = (openai as { components: { schemas: Record<string, Record<string, unknown>> } }).components.schemas;

const OPENAI_EVENTS: Record<string, string> = Object.fromEntries(
  ((schemas.ResponseStreamEvent.anyOf ?? schemas.ResponseStreamEvent.oneOf ?? []) as Array<{ $ref?: string }>)
    .map((m) => m.$ref?.split("/").pop() ?? "")
    .flatMap((name) => {
      const type = (schemas[name]?.properties as Record<string, { enum?: string[]; const?: string }> | undefined)?.type;
      const tag = type?.const ?? type?.enum?.[0];
      return tag ? [[tag, name]] : [];
    }),
);

export function streamed(route: string, event: Record<string, unknown>): string[] {
  if (route === "/chat/completions") return "error" in event ? schema("openai", "ErrorResponse", event) : schema("openai", "CreateChatCompletionStreamResponse", event);
  if (route === "/completions") return "error" in event ? schema("openai", "ErrorResponse", event) : schema("openai", "CreateCompletionResponse", event);
  const own = OPENAI_EVENTS[String(event.type)];
  const findings = own ? schema("openai", own, event) : [`openai has no Responses event of type ${String(event.type)}`];
  const open = OPEN_EVENTS[String(event.type)];
  if (open && !openAside(event)) findings.push(...schema("openresponses", open, openOnly(event)));
  return findings;
}

export function eventsOf(text: string): Array<{ event?: string; data: string }> {
  return text
    .split(/\n\n/)
    .map((block) => block.split("\n").filter((l) => !l.startsWith(":")))
    .filter((lines) => lines.some((l) => l.startsWith("data:")))
    .map((lines) => ({
      event: lines.find((l) => l.startsWith("event:"))?.slice(6).trim(),
      data: lines.filter((l) => l.startsWith("data:")).map((l) => l.slice(5).replace(/^ /, "")).join("\n"),
    }));
}

export function wire(found: string[], askedOf: Spec = "openai"): typeof fetch {
  return (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const route = routeOf(url);
    if (route && typeof init?.body === "string" && init.body.length > 0) {
      found.push(...asked(route, method, JSON.parse(init.body), askedOf).map((f) => `${method} ${route} asked: ${f}`));
    }
    const res = await fetch(input, init);
    if (!route || !res.body) return res;
    const type = res.headers.get("content-type") ?? "";
    const heard = (text: string) => {
      if (type.includes("text/event-stream")) {
        for (const { data } of eventsOf(text)) {
          if (data === "[DONE]") continue;
          found.push(...streamed(route, JSON.parse(data)).map((f) => `${method} ${route} streamed: ${f}`));
        }
      } else if (type.includes("application/json") && text.length > 0) {
        found.push(...answered(route, method, res.status, JSON.parse(text)).map((f) => `${method} ${route} ${res.status}: ${f}`));
      }
    };
    const decoder = new TextDecoder();
    let text = "";
    const body = res.body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, out) {
        text += decoder.decode(chunk, { stream: true });
        out.enqueue(chunk);
      },
      flush() {
        heard(text + decoder.decode());
      },
    }));
    return new Response(body, { status: res.status, statusText: res.statusText, headers: res.headers });
  }) as typeof fetch;
}
