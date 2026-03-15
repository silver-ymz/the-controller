const isTauri = typeof window !== "undefined" && !!(window as any).__TAURI_INTERNALS__;

function getAuthToken(): string | null {
  if (isTauri) return null;
  const params = new URLSearchParams(window.location.search);
  return params.get("token") || null;
}

const authToken = getAuthToken();

let sharedWs: WebSocket | null = null;
let wsListeners: Array<(msg: MessageEvent) => void> = [];
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let reconnectDelay = 1000;

function connectWebSocket(): WebSocket {
  const tokenParam = authToken ? `?token=${authToken}` : "";
  const wsUrl = `ws://${window.location.host}/ws${tokenParam}`;
  const ws = new WebSocket(wsUrl);

  ws.addEventListener("open", () => {
    reconnectDelay = 1000; // reset on success
  });

  ws.addEventListener("message", (msg) => {
    for (const listener of wsListeners) {
      listener(msg);
    }
  });

  ws.addEventListener("close", () => {
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      sharedWs = connectWebSocket();
    }, reconnectDelay);
    reconnectDelay = Math.min(reconnectDelay * 2, 30000);
  });

  return ws;
}

function getSharedWebSocket(): WebSocket {
  if (!sharedWs || sharedWs.readyState === WebSocket.CLOSED || sharedWs.readyState === WebSocket.CLOSING) {
    sharedWs = connectWebSocket();
  }
  return sharedWs;
}

export async function command<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (authToken) headers["Authorization"] = `Bearer ${authToken}`;
  const res = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers,
    body: JSON.stringify(args ?? {}),
  });
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

export function listen<T>(event: string, handler: (payload: T) => void): () => void {
  if (isTauri) {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen<T>(event, (e) => handler(e.payload)).then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }

  getSharedWebSocket(); // ensure connected
  const callback = (msg: MessageEvent) => {
    const data = JSON.parse(msg.data);
    if (data.event === event) handler(data.payload);
  };
  wsListeners.push(callback);
  return () => {
    wsListeners = wsListeners.filter((l) => l !== callback);
  };
}
