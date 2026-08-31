import * as vscode from 'vscode';

/** The server's ApiResponse envelope: `{ ok, data?, error? }`. */
export interface ApiResponse<T = any> {
  ok: boolean;
  data?: T;
  error?: string;
}

/** A person-held edit lock, as returned on reads. */
export interface EditLock {
  held_by: string;
  held_by_name: string;
  held_at: number;
  expires_at: number;
}

/** One row from `ListProgramsAll`. */
export interface ProgramObject {
  ref_id: string;
  key: string;
  title: string | null;
  kind: string;
  area: string | null;
  has_script: boolean;
  hooks: string[];
  libs: string[];
  locked: boolean;
  version?: number | null;
  lock?: EditLock | null;
}

/** `GetScript` payload (null when the object has no script). */
export interface Script {
  source: string;
  hooks: string[];
  enabled: boolean;
  version?: number | null;
  lock?: EditLock | null;
}

/** Who the token belongs to (`me`). */
export interface Identity {
  account_id: string;
  username: string;
  scopes: string[];
  email?: string | null;
  active_character?: string | null;
}

/** One entry of version history. */
export interface ScriptVersion {
  version: number;
  author: string;
  author_name: string;
  origin: string;
  created_at: number;
  hash: string;
  merged_from?: number | null;
}

/** Result of a versioned publish. */
export type PublishResult =
  | { kind: 'ok'; version: number; merged_from?: number | null; source: string }
  | { kind: 'conflict'; base: string; theirs: string; ours: string; current_version: number }
  | { kind: 'error'; message: string };

/**
 * Thin REST client for the Hearth `POST /api` endpoint. Reads `serverUrl` and
 * `token` from configuration on every call, so changing settings takes effect
 * without a reconnect. Uses the global `fetch` shipped with VS Code's Node 18+.
 */
export class HearthApi {
  private get config() {
    return vscode.workspace.getConfiguration('hearth');
  }

  get serverUrl(): string {
    return (this.config.get<string>('serverUrl') || 'http://localhost:8000').replace(/\/$/, '');
  }

  get token(): string {
    return this.config.get<string>('token') || '';
  }

  /** POST one action. Rejects on transport failure or `{ ok: false }`. */
  async call<T = any>(action: string, params: Record<string, unknown> = {}): Promise<T> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.token) {
      headers['Authorization'] = `Bearer ${this.token}`;
    }

    let res: Response;
    try {
      res = await fetch(`${this.serverUrl}/api`, {
        method: 'POST',
        headers,
        body: JSON.stringify({ action, ...params }),
      });
    } catch (e: any) {
      throw new Error(`Cannot reach Hearth at ${this.serverUrl}: ${e?.message ?? e}`);
    }

    // A request rejected before the handler (unknown action on an older
    // backend, or an auth extractor rejection) comes back as plain text, not
    // our JSON envelope — parse defensively, mirroring the web client.
    const body = await res.text();
    let parsed: ApiResponse<T>;
    try {
      parsed = JSON.parse(body);
    } catch {
      throw new Error(body.trim() || `HTTP ${res.status}`);
    }

    if (!parsed.ok) {
      throw new Error(parsed.error || `Action '${action}' failed`);
    }
    return parsed.data as T;
  }

  /** Like `call`, but returns the whole envelope instead of throwing on
   * `{ ok: false }` — needed for `set_script`, whose conflict is a non-ok
   * response carrying data. */
  private async callRaw(action: string, params: Record<string, unknown> = {}): Promise<ApiResponse> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`;
    let res: Response;
    try {
      res = await fetch(`${this.serverUrl}/api`, {
        method: 'POST',
        headers,
        body: JSON.stringify({ action, ...params }),
      });
    } catch (e: any) {
      return { ok: false, error: `Cannot reach Hearth at ${this.serverUrl}: ${e?.message ?? e}` };
    }
    const body = await res.text();
    try {
      return JSON.parse(body);
    } catch {
      return { ok: false, error: body.trim() || `HTTP ${res.status}` };
    }
  }

  listPrograms(): Promise<ProgramObject[]> {
    return this.call<ProgramObject[]>('list_programs_all');
  }

  me(): Promise<Identity> {
    return this.call<Identity>('me');
  }

  getScript(refId: string): Promise<Script | null> {
    return this.call<Script | null>('get_script', { ref_id: refId });
  }

  listLibs(refId: string): Promise<{ name: string; source: string; version?: number | null; lock?: EditLock | null }[]> {
    return this.call('list_libs', { ref_id: refId });
  }

  /** Versioned publish of an object script (name omitted) or a lib module. */
  async publish(
    refId: string,
    source: string,
    baseVersion: number | null,
    name?: string,
  ): Promise<PublishResult> {
    const action = name ? 'set_lib' : 'set_script';
    const params: Record<string, unknown> = { ref_id: refId, source, base_version: baseVersion };
    if (name) params.name = name;
    const res = await this.callRaw(action, params);
    if (res.ok) {
      return { kind: 'ok', version: res.data.version, merged_from: res.data.merged_from, source: res.data.source };
    }
    if (res.error === 'conflict' && res.data?.conflict) {
      return {
        kind: 'conflict',
        base: res.data.base,
        theirs: res.data.theirs,
        ours: res.data.ours,
        current_version: res.data.current_version,
      };
    }
    return { kind: 'error', message: res.error ?? 'publish failed' };
  }

  listScriptVersions(refId: string, name?: string): Promise<{ versions: ScriptVersion[] }> {
    return this.call('list_script_versions', { ref_id: refId, name });
  }

  getScriptVersion(refId: string, version: number, name?: string): Promise<{ source: string }> {
    return this.call('get_script_version', { ref_id: refId, version, name });
  }

  revertScript(refId: string, version: number, name?: string): Promise<{ version: number; source: string }> {
    return this.call('revert_script', { ref_id: refId, version, name });
  }

  lockScript(refId: string, name?: string): Promise<EditLock> {
    return this.call<EditLock>('lock_script', { ref_id: refId, name });
  }

  async unlockScript(refId: string, name?: string): Promise<void> {
    await this.call('unlock_script', { ref_id: refId, name });
  }

  /** Compile-check without running or saving. Returns `{ valid, error? }`. */
  checkProgram(source: string): Promise<{ valid: boolean; error?: string }> {
    return this.call('check_program', { source });
  }

  runTests(refId: string): Promise<TestResults> {
    return this.call<TestResults>('run_tests', { ref_id: refId });
  }
}

export interface TestResults {
  files: { file: string; tests: { name: string; passed: boolean; error?: string }[]; error?: string }[];
  passed: number;
  failed: number;
}
