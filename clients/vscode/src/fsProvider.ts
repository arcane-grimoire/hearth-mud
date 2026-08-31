import * as vscode from 'vscode';
import { EditLock, HearthApi } from './api';

export type Target =
  | { kind: 'obj'; refId: string }
  | { kind: 'lib'; refId: string; name: string };

/**
 * A read/write virtual filesystem over Hearth scripts, under the `hearth:`
 * scheme. Editing is git-like (see docs/plans/softcode-versioning.md): a save
 * (⌘S) updates the LOCAL working copy only — it never publishes. `hearth.publish`
 * is the explicit, versioned push to the server.
 *
 *   hearth:/obj/<ref_id>.luau              → an object's whole script
 *   hearth:/lib/<host_ref_id>/<name>.luau  → one lib module on a Code host
 *
 * The provider caches each opened doc's working copy plus the version it was
 * fetched at (the publish base), and tracks which docs have unpublished edits.
 */
export class HearthFsProvider implements vscode.FileSystemProvider {
  static readonly scheme = 'hearth';

  private readonly _emitter = new vscode.EventEmitter<vscode.FileChangeEvent[]>();
  readonly onDidChangeFile = this._emitter.event;

  private readonly cache = new Map<string, string>();
  private readonly baseVersion = new Map<string, number | null>();
  private readonly locks = new Map<string, EditLock | null>();
  private readonly unpublished = new Set<string>();

  constructor(private readonly api: HearthApi) {}

  static objectUri(refId: string): vscode.Uri {
    return vscode.Uri.parse(`${HearthFsProvider.scheme}:/obj/${encodeURIComponent(refId)}.luau`);
  }
  static libUri(hostRefId: string, name: string): vscode.Uri {
    return vscode.Uri.parse(
      `${HearthFsProvider.scheme}:/lib/${encodeURIComponent(hostRefId)}/${encodeURIComponent(name)}.luau`,
    );
  }

  static uriFor(t: Target): vscode.Uri {
    return t.kind === 'obj' ? HearthFsProvider.objectUri(t.refId) : HearthFsProvider.libUri(t.refId, t.name);
  }

  targetOf(uri: vscode.Uri): Target | null {
    const parts = uri.path.replace(/^\//, '').split('/');
    if (parts[0] === 'obj' && parts.length === 2 && parts[1].endsWith('.luau')) {
      return { kind: 'obj', refId: decodeURIComponent(parts[1].slice(0, -'.luau'.length)) };
    }
    if (parts[0] === 'lib' && parts.length === 3 && parts[2].endsWith('.luau')) {
      return {
        kind: 'lib',
        refId: decodeURIComponent(parts[1]),
        name: decodeURIComponent(parts[2].slice(0, -'.luau'.length)),
      };
    }
    return null;
  }

  // --- state accessors used by the publish/history commands ---

  baseVersionOf(uri: vscode.Uri): number | null {
    return this.baseVersion.get(uri.toString()) ?? null;
  }
  setBaseVersion(uri: vscode.Uri, version: number | null): void {
    this.baseVersion.set(uri.toString(), version);
  }
  isUnpublished(uri: vscode.Uri): boolean {
    return this.unpublished.has(uri.toString());
  }
  markPublished(uri: vscode.Uri): void {
    this.unpublished.delete(uri.toString());
  }
  contentOf(uri: vscode.Uri): string {
    return this.cache.get(uri.toString()) ?? '';
  }
  lockOf(uri: vscode.Uri): EditLock | null {
    return this.locks.get(uri.toString()) ?? null;
  }

  /** Fetch fresh from the server, seed the cache + base version, and open it. */
  async open(t: Target): Promise<void> {
    const uri = HearthFsProvider.uriFor(t);
    let source = '';
    let version: number | null = null;
    let lock: EditLock | null = null;
    if (t.kind === 'obj') {
      const script = await this.api.getScript(t.refId);
      source = script?.source ?? '';
      version = script?.version ?? null;
      lock = script?.lock ?? null;
    } else {
      const libs = await this.api.listLibs(t.refId);
      const found = libs.find((l) => l.name === t.name);
      if (!found) throw new Error(`No library '${t.name}' on ${t.refId}`);
      source = found.source;
      version = found.version ?? null;
      lock = found.lock ?? null;
    }
    const key = uri.toString();
    this.cache.set(key, source);
    this.baseVersion.set(key, version);
    this.locks.set(key, lock);
    this.unpublished.delete(key);
    this._emitter.fire([{ type: vscode.FileChangeType.Changed, uri }]);

    const doc = await vscode.workspace.openTextDocument(uri);
    if (doc.languageId !== 'luau') await vscode.languages.setTextDocumentLanguage(doc, 'luau');
    await vscode.window.showTextDocument(doc, { preview: false });
  }

  // --- FileSystemProvider ---

  async stat(uri: vscode.Uri): Promise<vscode.FileStat> {
    const size = new TextEncoder().encode(this.contentOf(uri)).byteLength;
    return { type: vscode.FileType.File, ctime: 0, mtime: Date.now(), size };
  }

  async readFile(uri: vscode.Uri): Promise<Uint8Array> {
    const key = uri.toString();
    if (!this.cache.has(key)) {
      // Cold read (e.g. a diff revision) — fetch without disturbing edit state.
      const t = this.targetOf(uri);
      if (t) {
        try {
          await this.open(t);
        } catch {
          /* fall through to empty */
        }
      }
    }
    return new TextEncoder().encode(this.cache.get(key) ?? '');
  }

  async writeFile(uri: vscode.Uri, content: Uint8Array): Promise<void> {
    // Local save only — never publishes. `hearth.publish` pushes to the server.
    const key = uri.toString();
    this.cache.set(key, new TextDecoder().decode(content));
    this.unpublished.add(key);
    this._emitter.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  }

  watch(): vscode.Disposable {
    return new vscode.Disposable(() => {});
  }
  readDirectory(): [string, vscode.FileType][] {
    return [];
  }
  createDirectory(): void {}
  delete(uri: vscode.Uri): void {
    throw vscode.FileSystemError.NoPermissions(uri);
  }
  rename(uri: vscode.Uri): void {
    throw vscode.FileSystemError.NoPermissions(uri);
  }
}
