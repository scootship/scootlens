// 薄 JSON-RPC 2.0 over WebSocket 客户端。与 ScootLens ABI 同构（docs/03-abi-spec.md）。
// socket 通过工厂注入，便于在 vitest 中用假 socket 完整覆盖，无需真实网络。

export type RpcId = number | string;

export interface RpcRequest {
  jsonrpc: "2.0";
  id: RpcId;
  method: string;
  params?: unknown;
}

export interface RpcErrorPayload {
  code: number;
  message: string;
  data?: unknown;
}

export interface RpcSuccess {
  jsonrpc: "2.0";
  id: RpcId;
  result: unknown;
}

export interface RpcFailure {
  jsonrpc: "2.0";
  id: RpcId;
  error: RpcErrorPayload;
}

export type RpcFrame = RpcSuccess | RpcFailure | RpcRequest;

/** 结构化 RPC 错误（携带 ABI 错误码，如 `E_CAP_DENIED`）。 */
export class RpcError extends Error {
  readonly code: number;
  readonly data: unknown;
  constructor(payload: RpcErrorPayload) {
    super(payload.message);
    this.name = "RpcError";
    this.code = payload.code;
    this.data = payload.data;
  }
  /** ABI 错误码字符串（`error.data.code`），如 `E_CAP_DENIED`。 */
  get abiCode(): string | undefined {
    const d = this.data;
    if (d && typeof d === "object" && "code" in d) {
      const c = (d as { code: unknown }).code;
      return typeof c === "string" ? c : undefined;
    }
    return undefined;
  }
}

/** WebSocket 的最小结构契约，便于测试替身实现。 */
export interface SocketLike {
  send(data: string): void;
  close(): void;
  onopen: ((this: unknown, ev?: unknown) => unknown) | null;
  onclose: ((this: unknown, ev?: unknown) => unknown) | null;
  onerror: ((this: unknown, ev?: unknown) => unknown) | null;
  onmessage: ((this: unknown, ev: { data: unknown }) => unknown) | null;
}

export type SocketFactory = (url: string) => SocketLike;

export type ConnState = "idle" | "connecting" | "open" | "closed";

export type EventHandler = (method: string, params: unknown) => void;
export type StateHandler = (state: ConnState) => void;

const defaultFactory: SocketFactory = (url) =>
  new WebSocket(url) as unknown as SocketLike;

/** 用 slt1 令牌构造握手 URL：`<base>/ws?token=<token>`。 */
export function handshakeUrl(base: string, token: string): string {
  const sep = base.endsWith("/") ? "" : "/";
  return `${base}${sep}ws?token=${encodeURIComponent(token)}`;
}

export class RpcClient {
  private seq = 0;
  private socket: SocketLike | null = null;
  private readonly pending = new Map<
    RpcId,
    { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  >();
  private readonly events = new Set<EventHandler>();
  private readonly states = new Set<StateHandler>();
  private connState: ConnState = "idle";

  constructor(
    private readonly url: string,
    private readonly factory: SocketFactory = defaultFactory,
  ) {}

  get state(): ConnState {
    return this.connState;
  }

  private setState(s: ConnState): void {
    this.connState = s;
    for (const h of this.states) h(s);
  }

  onState(handler: StateHandler): () => void {
    this.states.add(handler);
    return () => this.states.delete(handler);
  }

  onEvent(handler: EventHandler): () => void {
    this.events.add(handler);
    return () => this.events.delete(handler);
  }

  connect(): Promise<void> {
    if (this.connState === "open" || this.connState === "connecting") {
      return Promise.resolve();
    }
    this.setState("connecting");
    const sock = this.factory(this.url);
    this.socket = sock;
    return new Promise((resolve, reject) => {
      sock.onopen = () => {
        this.setState("open");
        resolve();
      };
      sock.onmessage = (ev) => this.handleMessage(ev.data);
      sock.onerror = (err) => {
        if (this.connState === "connecting") reject(err);
      };
      sock.onclose = () => {
        this.setState("closed");
        this.failAll(new Error("connection closed"));
      };
    });
  }

  private handleMessage(data: unknown): void {
    if (typeof data !== "string") return;
    let frame: RpcFrame;
    try {
      frame = JSON.parse(data) as RpcFrame;
    } catch {
      return;
    }
    // Server notification（如 `evt.event`）：有 method、无对应 pending。
    if ("method" in frame && !("result" in frame) && !("error" in frame)) {
      for (const h of this.events) h(frame.method, frame.params);
      return;
    }
    const id = (frame as RpcSuccess | RpcFailure).id;
    const waiter = this.pending.get(id);
    if (!waiter) return;
    this.pending.delete(id);
    if ("error" in frame) {
      waiter.reject(new RpcError((frame as RpcFailure).error));
    } else {
      waiter.resolve((frame as RpcSuccess).result);
    }
  }

  private failAll(err: unknown): void {
    for (const [, w] of this.pending) w.reject(err);
    this.pending.clear();
  }

  /** 发起一次 RPC 调用，解析出 `result`；失败抛 {@link RpcError}。 */
  call<T = unknown>(method: string, params?: unknown): Promise<T> {
    if (!this.socket || this.connState !== "open") {
      return Promise.reject(new Error("not connected"));
    }
    const id = ++this.seq;
    const req: RpcRequest = { jsonrpc: "2.0", id, method };
    if (params !== undefined) req.params = params;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (v) => resolve(v as T),
        reject,
      });
      try {
        this.socket!.send(JSON.stringify(req));
      } catch (e) {
        this.pending.delete(id);
        reject(e);
      }
    });
  }

  close(): void {
    this.socket?.close();
    this.socket = null;
    this.setState("closed");
  }
}
