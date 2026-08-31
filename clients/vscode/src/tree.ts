import * as vscode from 'vscode';
import { HearthApi, ProgramObject } from './api';
import { HearthFsProvider } from './fsProvider';

/**
 * Explorer tree: areas → objects → lib modules. One `list_programs_all` call
 * carries each object's version + edit lock, so the tree renders lock holders
 * and unpublished markers without any per-node request.
 */
export class ProgramsTree implements vscode.TreeDataProvider<Node> {
  private readonly _emitter = new vscode.EventEmitter<Node | undefined | void>();
  readonly onDidChangeTreeData = this._emitter.event;

  private objects: ProgramObject[] = [];

  constructor(
    private readonly api: HearthApi,
    private readonly fs: HearthFsProvider,
  ) {}

  refresh(): void {
    this.objects = [];
    this._emitter.fire();
  }

  getTreeItem(node: Node): vscode.TreeItem {
    return node.item;
  }

  async getChildren(node?: Node): Promise<Node[]> {
    if (!node) {
      if (this.objects.length === 0) {
        try {
          this.objects = await this.api.listPrograms();
        } catch (e: any) {
          return [{ kind: 'message', item: new vscode.TreeItem(`⚠ ${e?.message ?? e}`) }];
        }
      }
      const areas = [...new Set(this.objects.map((o) => o.area ?? '(no area)'))].sort();
      return areas.map((area) => {
        const item = new vscode.TreeItem(area, vscode.TreeItemCollapsibleState.Collapsed);
        item.iconPath = new vscode.ThemeIcon('folder');
        return { kind: 'area', area, item };
      });
    }

    if (node.kind === 'area') {
      return this.objects
        .filter((o) => (o.area ?? '(no area)') === node.area)
        .sort((a, b) => a.ref_id.localeCompare(b.ref_id))
        .map((o) => this.objectNode(o));
    }

    if (node.kind === 'object' && node.obj.libs.length > 0) {
      return node.obj.libs.map((name) => {
        const uri = HearthFsProvider.libUri(node.obj.ref_id, name);
        const item = new vscode.TreeItem(`${name}.luau`, vscode.TreeItemCollapsibleState.None);
        item.iconPath = new vscode.ThemeIcon('library');
        item.contextValue = 'hearthLib';
        if (this.fs.isUnpublished(uri)) item.description = '● unpublished';
        item.command = { command: 'hearth.openLib', title: 'Open Library', arguments: [node.obj.ref_id, name] };
        return { kind: 'lib', item };
      });
    }

    return [];
  }

  private objectNode(o: ProgramObject): Node {
    const hasChildren = o.libs.length > 0;
    const item = new vscode.TreeItem(
      o.title || o.key,
      hasChildren ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    const uri = HearthFsProvider.objectUri(o.ref_id);
    const bits: string[] = [];
    if (this.fs.isUnpublished(uri)) bits.push('●');
    bits.push(o.ref_id);
    if (o.lock) bits.push(`🔒 ${o.lock.held_by_name}`);
    else if (o.version) bits.push(`v${o.version}`);
    item.description = bits.join(' · ');
    item.contextValue = 'hearthObject';
    item.iconPath = new vscode.ThemeIcon(o.locked ? 'lock' : iconForKind(o.kind));
    item.tooltip =
      `${o.kind} ${o.ref_id}` +
      (o.locked ? ' (locked — read-only)' : '') +
      (o.lock ? ` — edit lock held by ${o.lock.held_by_name}` : '');
    if (o.has_script) {
      item.command = { command: 'hearth.openObject', title: 'Open Script', arguments: [o.ref_id] };
    }
    return { kind: 'object', obj: o, item };
  }
}

function iconForKind(kind: string): string {
  switch (kind) {
    case 'Room':
      return 'home';
    case 'Npc':
      return 'person';
    case 'Item':
      return 'package';
    case 'Exit':
      return 'arrow-right';
    case 'Code':
      return 'file-code';
    default:
      return 'symbol-object';
  }
}

export type Node =
  | { kind: 'area'; area: string; item: vscode.TreeItem }
  | { kind: 'object'; obj: ProgramObject; item: vscode.TreeItem }
  | { kind: 'lib'; item: vscode.TreeItem }
  | { kind: 'message'; item: vscode.TreeItem };
