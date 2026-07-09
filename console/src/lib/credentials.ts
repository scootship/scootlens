// Console-side credential bindings. Secrets stay in kernel vault; this registry
// stores only origin patterns and vault_ref handles used for explicit fills.

const KEY = "scootlens.credentials";

export interface CredentialStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface CredentialProfile {
  id: string;
  label: string;
  origin: string;
  usernameRef: string;
  passwordRef: string;
  loginUrl?: string;
}

export interface CredentialDraft {
  id?: string;
  label?: string;
  origin: string;
  usernameRef: string;
  passwordRef: string;
  loginUrl?: string;
}

function resolve(store?: CredentialStore): CredentialStore | null {
  if (store) return store;
  try {
    if (typeof localStorage !== "undefined") return localStorage;
  } catch {
    // localStorage can throw in restricted browser contexts.
  }
  return null;
}

function stableId(label: string, origin: string): string {
  return `${origin}::${label}`
    .toLowerCase()
    .replace(/[^a-z0-9*_.:-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 96);
}

export function normalizeOriginPattern(raw: string): string {
  const input = raw.trim().toLowerCase().replace(/^@/, "");
  if (!input) throw new Error("origin 不能为空");
  if (input === "*") throw new Error("凭据 origin 不允许使用 *");

  let origin = input;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(input)) {
    try {
      origin = new URL(input).host.toLowerCase();
    } catch {
      throw new Error(`origin 无法解析: ${raw}`);
    }
  }

  origin = origin.replace(/\/+$/, "");
  if (!origin || origin.includes("/") || origin.includes("@")) {
    throw new Error(`origin 只能是 host、host:port 或 *.suffix: ${raw}`);
  }
  if (origin.startsWith("*.")) {
    const suffix = origin.slice(2);
    if (!suffix || suffix.includes("*")) {
      throw new Error(`origin 通配符格式无效: ${raw}`);
    }
  } else if (origin.includes("*")) {
    throw new Error(`origin 只支持前缀通配符 *.suffix: ${raw}`);
  }
  return origin;
}

export function originFromUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    return new URL(url).host.toLowerCase();
  } catch {
    return null;
  }
}

export function originMatches(pattern: string, urlOrOrigin: string | null | undefined): boolean {
  const origin = originFromUrl(urlOrOrigin) ?? urlOrOrigin?.trim().toLowerCase() ?? "";
  if (!origin) return false;
  const p = normalizeOriginPattern(pattern);
  if (p.startsWith("*.")) {
    const suffix = p.slice(2);
    return origin.endsWith(`.${suffix}`);
  }
  return origin === p;
}

function asCredential(v: unknown): CredentialProfile | null {
  if (!v || typeof v !== "object") return null;
  const r = v as Record<string, unknown>;
  const origin = typeof r.origin === "string" ? r.origin : "";
  const usernameRef = typeof r.usernameRef === "string" ? r.usernameRef.trim() : "";
  const passwordRef = typeof r.passwordRef === "string" ? r.passwordRef.trim() : "";
  if (!origin || !usernameRef || !passwordRef) return null;
  try {
    const normalized = normalizeOriginPattern(origin);
    const label =
      typeof r.label === "string" && r.label.trim() ? r.label.trim() : normalized;
    const id =
      typeof r.id === "string" && r.id.trim()
        ? r.id.trim()
        : stableId(label, normalized);
    const loginUrl = typeof r.loginUrl === "string" && r.loginUrl.trim() ? r.loginUrl.trim() : undefined;
    return { id, label, origin: normalized, usernameRef, passwordRef, loginUrl };
  } catch {
    return null;
  }
}

function read(store: CredentialStore | null): CredentialProfile[] {
  if (!store) return [];
  try {
    const raw = store.getItem(KEY);
    if (!raw) return [];
    const arr: unknown = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr
      .map(asCredential)
      .filter((c): c is CredentialProfile => c !== null)
      .sort(sortCredential);
  } catch {
    return [];
  }
}

function sortCredential(a: CredentialProfile, b: CredentialProfile): number {
  return a.origin.localeCompare(b.origin) || a.label.localeCompare(b.label);
}

function write(store: CredentialStore | null, credentials: CredentialProfile[]): CredentialProfile[] {
  const sorted = [...credentials].sort(sortCredential);
  if (store) {
    try {
      store.setItem(KEY, JSON.stringify(sorted));
    } catch {
      // Ignore storage failures; callers can still use the returned list this render.
    }
  }
  return sorted;
}

export function listCredentials(store?: CredentialStore): CredentialProfile[] {
  return read(resolve(store));
}

export function saveCredential(draft: CredentialDraft, store?: CredentialStore): CredentialProfile[] {
  const s = resolve(store);
  const origin = normalizeOriginPattern(draft.origin);
  const usernameRef = draft.usernameRef.trim();
  const passwordRef = draft.passwordRef.trim();
  if (!usernameRef) throw new Error("请填写用户名 vault_ref");
  if (!passwordRef) throw new Error("请填写密码 vault_ref");
  const label = draft.label?.trim() || origin;
  const loginUrl = draft.loginUrl?.trim() || undefined;
  if (loginUrl) {
    try {
      new URL(loginUrl);
    } catch {
      throw new Error("登录页 URL 无法解析");
    }
  }

  const credential: CredentialProfile = {
    id: draft.id?.trim() || stableId(label, origin),
    label,
    origin,
    usernameRef,
    passwordRef,
    loginUrl,
  };
  const next = [...read(s).filter((c) => c.id !== credential.id), credential];
  return write(s, next);
}

export function forgetCredential(id: string, store?: CredentialStore): CredentialProfile[] {
  const s = resolve(store);
  return write(s, read(s).filter((c) => c.id !== id));
}

export function matchingCredentials(
  url: string | null | undefined,
  credentials: CredentialProfile[],
): CredentialProfile[] {
  return credentials.filter((c) => originMatches(c.origin, url));
}
